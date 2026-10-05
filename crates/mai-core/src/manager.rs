//! Runs two tasks per host (probe connection and terminals) plus one
//! monitor task, and gives the app a small handle to add, remove and
//! command hosts and to open terminals. Every change comes back as an
//! `Update` on the receiver returned by `HostManager::start`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mai_protocol::AppMsg;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::host::{Connector, HostCommand, HostConfig, HostEvent, run_host};
use crate::monitor::{Monitor, Update};
use crate::pty::TermSize;
use crate::term::{AttachSpec, TermCmd, TermError, TermEvent, ZellijPaths, run_terms};
use crate::tracker::{AgentKey, TrackerConfig};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

enum Ctl {
    Added(String),
    Removed(String),
    Acknowledge(AgentKey),
}

/// The two tasks of one host.
struct HostTasks {
    probe: UnboundedSender<HostCommand>,
    term: UnboundedSender<TermCmd>,
}

/// Handle to the running hosts. Dropping it stops every host task and,
/// once they have ended, the monitor task (the update stream then ends).
pub struct HostManager<C> {
    connector: Arc<C>,
    hosts: HashMap<String, HostTasks>,
    events: UnboundedSender<(String, HostEvent)>,
    ctl: UnboundedSender<Ctl>,
    zellij: ZellijPaths,
}

/// An open terminal: `zellij attach` of one session. Dropping it detaches.
pub struct Terminal {
    pub host: String,
    pub id: u64,
    events: UnboundedReceiver<TermEvent>,
    cmds: UnboundedSender<TermCmd>,
}

impl Terminal {
    /// Keyboard input (bytes as the terminal emulator produces them).
    pub fn write(&self, data: Vec<u8>) {
        let _ = self.cmds.send(TermCmd::Input { id: self.id, data });
    }

    pub fn resize(&self, size: TermSize) {
        let _ = self.cmds.send(TermCmd::Resize { id: self.id, size });
    }

    /// Next event; `None` once the terminal is finished (after
    /// `TermEvent::Exited`, or when its host was removed).
    pub async fn recv(&mut self) -> Option<TermEvent> {
        self.events.recv().await
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.cmds.send(TermCmd::Close { id: self.id });
    }
}

async fn run_monitor(
    mut monitor: Monitor,
    mut events: UnboundedReceiver<(String, HostEvent)>,
    mut ctl: UnboundedReceiver<Ctl>,
    updates: UnboundedSender<Update>,
    zellij: ZellijPaths,
) {
    // Events of a removed host that were still queued are dropped.
    let mut removed: HashSet<String> = HashSet::new();
    let mut ctl_open = true;
    loop {
        // Biased: a control message sent before a host event (e.g. a
        // host re-added before its task starts) is seen first.
        let out = tokio::select! {
            biased;
            c = ctl.recv(), if ctl_open => match c {
                None => {
                    ctl_open = false;
                    continue;
                }
                Some(Ctl::Added(id)) => {
                    removed.remove(&id);
                    continue;
                }
                Some(Ctl::Removed(id)) => {
                    monitor.remove_host(&id);
                    zellij.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
                    removed.insert(id);
                    continue;
                }
                Some(Ctl::Acknowledge(key)) => monitor.acknowledge(&key).into_iter().collect(),
            },
            ev = events.recv() => match ev {
                None => return,
                Some((id, _)) if removed.contains(&id) => continue,
                Some((id, ev)) => {
                    // Terminals attach with the zellij the probe found.
                    if let HostEvent::Hello(info) = &ev {
                        let mut paths = zellij.lock().unwrap_or_else(|p| p.into_inner());
                        match &info.zellij_path {
                            Some(p) => paths.insert(id.clone(), p.clone()),
                            None => paths.remove(&id),
                        };
                    }
                    monitor.apply(&id, ev, now_ms())
                }
            },
        };
        for u in out {
            if updates.send(u).is_err() {
                return;
            }
        }
    }
}

impl<C: Connector> HostManager<C> {
    /// Start the monitor task. Must be called inside a tokio runtime.
    pub fn start(connector: Arc<C>, cfg: TrackerConfig) -> (Self, UnboundedReceiver<Update>) {
        let (events, events_rx) = mpsc::unbounded_channel();
        let (ctl, ctl_rx) = mpsc::unbounded_channel();
        let (updates, updates_rx) = mpsc::unbounded_channel();
        let zellij = ZellijPaths::default();
        tokio::spawn(run_monitor(
            Monitor::new(cfg),
            events_rx,
            ctl_rx,
            updates,
            zellij.clone(),
        ));
        let manager = Self {
            connector,
            hosts: HashMap::new(),
            events,
            ctl,
            zellij,
        };
        (manager, updates_rx)
    }

    /// Start monitoring `cfg`. False if a host with this id exists.
    pub fn add_host(&mut self, cfg: HostConfig) -> bool {
        if self.hosts.contains_key(&cfg.id) {
            return false;
        }
        let (probe, probe_rx) = mpsc::unbounded_channel();
        let (term, term_rx) = mpsc::unbounded_channel();
        let _ = self.ctl.send(Ctl::Added(cfg.id.clone()));
        self.hosts.insert(cfg.id.clone(), HostTasks { probe, term });
        tokio::spawn(run_terms(
            cfg.clone(),
            self.connector.clone(),
            self.zellij.clone(),
            self.events.clone(),
            term_rx,
        ));
        tokio::spawn(run_host(
            cfg,
            self.connector.clone(),
            self.events.clone(),
            probe_rx,
        ));
        true
    }

    /// Stop monitoring host `id`, close its terminals and forget its state.
    pub fn remove_host(&mut self, id: &str) -> bool {
        let Some(tasks) = self.hosts.remove(id) else {
            return false;
        };
        let _ = tasks.probe.send(HostCommand::Stop);
        let _ = tasks.term.send(TermCmd::Stop);
        let _ = self.ctl.send(Ctl::Removed(id.to_owned()));
        true
    }

    /// Reconnect host `id` now (after a failure that needs the user, or
    /// to skip a backoff wait); applies to both connections.
    pub fn retry(&self, id: &str) -> bool {
        let Some(tasks) = self.hosts.get(id) else {
            return false;
        };
        let _ = tasks.term.send(TermCmd::Retry);
        tasks.probe.send(HostCommand::Retry).is_ok()
    }

    /// Send `msg` to host `id`'s probe. If the probe is down, an
    /// `Update::Dropped` reports it.
    pub fn send(&self, id: &str, msg: AppMsg) -> bool {
        self.hosts
            .get(id)
            .is_some_and(|t| t.probe.send(HostCommand::Send(msg)).is_ok())
    }

    /// The user has seen this agent's alert.
    pub fn acknowledge(&self, key: AgentKey) {
        let _ = self.ctl.send(Ctl::Acknowledge(key));
    }

    /// Attach a terminal to zellij session `session` on host `host`
    /// (creating the session first if `create`). The first terminal of a
    /// host opens its terminal connection; this may prompt (host key,
    /// password) like the probe connection.
    pub async fn open_terminal(
        &self,
        host: &str,
        session: &str,
        create: bool,
        size: TermSize,
    ) -> Result<Terminal, TermError> {
        let tasks = self.hosts.get(host).ok_or(TermError::NoHost)?;
        let (reply, answer) = oneshot::channel();
        let spec = AttachSpec {
            session: session.to_owned(),
            create,
            size,
        };
        tasks
            .term
            .send(TermCmd::Open { spec, reply })
            .map_err(|_| TermError::Stopped)?;
        let (id, events) = answer.await.map_err(|_| TermError::Stopped)??;
        Ok(Terminal {
            host: host.to_owned(),
            id,
            events,
            cmds: tasks.term.clone(),
        })
    }

    pub fn host_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.hosts.keys().cloned().collect();
        ids.sort();
        ids
    }
}

impl<C> Drop for HostManager<C> {
    /// Stops every host's tasks. Open terminals hold a sender of their
    /// terminal task, so closing the channels alone would not end it.
    fn drop(&mut self) {
        for tasks in self.hosts.values() {
            let _ = tasks.probe.send(HostCommand::Stop);
            let _ = tasks.term.send(TermCmd::Stop);
        }
    }
}
