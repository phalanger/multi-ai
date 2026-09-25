//! Runs one task per host plus one monitor task, and gives the app a
//! small handle to add, remove and command hosts. Every change comes
//! back as an `Update` on the receiver returned by `HostManager::start`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mai_protocol::AppMsg;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::host::{Connector, HostCommand, HostConfig, HostEvent, run_host};
use crate::monitor::{Monitor, Update};
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

/// Handle to the running hosts. Dropping it stops every host task and,
/// once they have ended, the monitor task (the update stream then ends).
pub struct HostManager<C> {
    connector: Arc<C>,
    hosts: HashMap<String, UnboundedSender<HostCommand>>,
    events: UnboundedSender<(String, HostEvent)>,
    ctl: UnboundedSender<Ctl>,
}

async fn run_monitor(
    mut monitor: Monitor,
    mut events: UnboundedReceiver<(String, HostEvent)>,
    mut ctl: UnboundedReceiver<Ctl>,
    updates: UnboundedSender<Update>,
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
                    removed.insert(id);
                    continue;
                }
                Some(Ctl::Acknowledge(key)) => monitor.acknowledge(&key).into_iter().collect(),
            },
            ev = events.recv() => match ev {
                None => return,
                Some((id, _)) if removed.contains(&id) => continue,
                Some((id, ev)) => monitor.apply(&id, ev, now_ms()),
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
        tokio::spawn(run_monitor(Monitor::new(cfg), events_rx, ctl_rx, updates));
        let manager = Self {
            connector,
            hosts: HashMap::new(),
            events,
            ctl,
        };
        (manager, updates_rx)
    }

    /// Start monitoring `cfg`. False if a host with this id exists.
    pub fn add_host(&mut self, cfg: HostConfig) -> bool {
        if self.hosts.contains_key(&cfg.id) {
            return false;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = self.ctl.send(Ctl::Added(cfg.id.clone()));
        self.hosts.insert(cfg.id.clone(), tx);
        tokio::spawn(run_host(
            cfg,
            self.connector.clone(),
            self.events.clone(),
            rx,
        ));
        true
    }

    /// Stop monitoring host `id` and forget its state.
    pub fn remove_host(&mut self, id: &str) -> bool {
        let Some(tx) = self.hosts.remove(id) else {
            return false;
        };
        let _ = tx.send(HostCommand::Stop);
        let _ = self.ctl.send(Ctl::Removed(id.to_owned()));
        true
    }

    /// Reconnect host `id` now (after a failure that needs the user, or
    /// to skip a backoff wait).
    pub fn retry(&self, id: &str) -> bool {
        self.command(id, HostCommand::Retry)
    }

    /// Send `msg` to host `id`'s probe. If the probe is down, an
    /// `Update::Dropped` reports it.
    pub fn send(&self, id: &str, msg: AppMsg) -> bool {
        self.command(id, HostCommand::Send(msg))
    }

    /// The user has seen this agent's alert.
    pub fn acknowledge(&self, key: AgentKey) {
        let _ = self.ctl.send(Ctl::Acknowledge(key));
    }

    pub fn host_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.hosts.keys().cloned().collect();
        ids.sort();
        ids
    }

    fn command(&self, id: &str, cmd: HostCommand) -> bool {
        self.hosts.get(id).is_some_and(|tx| tx.send(cmd).is_ok())
    }
}
