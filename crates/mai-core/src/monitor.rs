//! Aggregates the events of every host into one view: host states, zellij
//! sessions and panes, agent states (via `Tracker`) and alerts. Pure and
//! synchronous; the manager task feeds it and forwards its `Update`s.

use std::collections::{BTreeMap, HashMap};

use mai_protocol::{AgentState, AppMsg, Metrics, PaneInfo, ProbeMsg, SessionInfo};

use crate::deploy::HookResult;
use crate::host::{ConnState, HostEvent, HostState, host_state};
use crate::link::HelloInfo;
use crate::tracker::{AgentKey, AgentRecord, Alert, Tracker, TrackerConfig};

/// A change the UI should show.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    Host {
        id: String,
        state: HostState,
        probe: ConnState,
    },
    Hello {
        id: String,
        info: HelloInfo,
    },
    Hooks {
        id: String,
        result: Result<Vec<HookResult>, String>,
    },
    Sessions {
        id: String,
        sessions: Vec<SessionInfo>,
    },
    Panes {
        id: String,
        session: String,
        panes: Vec<PaneInfo>,
    },
    /// An agent's state or acknowledgement changed.
    Agent(AgentRecord),
    /// The user should be notified.
    Alert(Alert),
    Metrics {
        id: String,
        metrics: Metrics,
    },
    ProbeError {
        id: String,
        code: String,
        message: String,
    },
    /// A command could not be delivered because the probe was down.
    Dropped {
        id: String,
        msg: AppMsg,
    },
}

/// What is known about one host.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostView {
    pub probe: Option<ConnState>,
    pub hello: Option<HelloInfo>,
    pub sessions: Vec<SessionInfo>,
    pub panes: BTreeMap<String, Vec<PaneInfo>>,
    pub metrics: Option<Metrics>,
}

pub struct Monitor {
    tracker: Tracker,
    hosts: HashMap<String, HostView>,
}

impl Monitor {
    pub fn new(cfg: TrackerConfig) -> Self {
        Self {
            tracker: Tracker::new(cfg),
            hosts: HashMap::new(),
        }
    }

    pub fn tracker(&self) -> &Tracker {
        &self.tracker
    }

    pub fn host(&self, id: &str) -> Option<&HostView> {
        self.hosts.get(id)
    }

    /// Mark an agent's alert as seen (the user opened its pane).
    pub fn acknowledge(&mut self, key: &AgentKey) -> Option<Update> {
        let was = self.tracker.get(key)?.acknowledged;
        self.tracker.acknowledge(key);
        let rec = self.tracker.get(key)?;
        (!was).then(|| Update::Agent(rec.clone()))
    }

    /// Forget a host that was removed from the configuration.
    pub fn remove_host(&mut self, id: &str) {
        self.hosts.remove(id);
    }

    pub fn apply(&mut self, id: &str, ev: HostEvent, now_ms: u64) -> Vec<Update> {
        let id = id.to_owned();
        let view = self.hosts.entry(id.clone()).or_default();
        match ev {
            HostEvent::Probe(probe) => {
                view.probe = Some(probe.clone());
                vec![Update::Host {
                    state: host_state(&probe, None),
                    id,
                    probe,
                }]
            }
            HostEvent::Hello(info) => {
                view.hello = Some(info.clone());
                vec![Update::Hello { id, info }]
            }
            HostEvent::Hooks(result) => vec![Update::Hooks { id, result }],
            HostEvent::Dropped(msg) => vec![Update::Dropped { id, msg }],
            HostEvent::Msg(msg) => self.apply_msg(id, msg, now_ms),
        }
    }

    fn apply_msg(&mut self, id: String, msg: ProbeMsg, now_ms: u64) -> Vec<Update> {
        let view = self.hosts.entry(id.clone()).or_default();
        match msg {
            ProbeMsg::Sessions { sessions } => {
                view.panes
                    .retain(|name, _| sessions.iter().any(|s| &s.name == name && !s.exited));
                view.sessions = sessions.clone();
                let live =
                    |session: &str, _: u32| sessions.iter().any(|s| s.name == session && !s.exited);
                let mut out = vec![Update::Sessions {
                    id: id.clone(),
                    sessions: sessions.clone(),
                }];
                out.extend(self.agents_gone(&id, None, live, now_ms));
                out
            }
            ProbeMsg::Panes { session, panes } => {
                view.panes.insert(session.clone(), panes.clone());
                let live = |_: &str, pane: u32| panes.iter().any(|p| p.id == pane && !p.exited);
                let mut out = vec![Update::Panes {
                    id: id.clone(),
                    session: session.clone(),
                    panes: panes.clone(),
                }];
                out.extend(self.agents_gone(&id, Some(&session), live, now_ms));
                out
            }
            ProbeMsg::AgentEvent(ev) => {
                let key = AgentKey::from_event(&id, &ev);
                let summary = |r: &AgentRecord| (r.state, r.acknowledged);
                let before = self.tracker.get(&key).map(summary);
                let alert = self.tracker.apply(&id, &ev);
                let mut out = Vec::new();
                // A new agent, a new state or a new unacknowledged alert.
                if let Some(rec) = self.tracker.get(&key)
                    && before != Some(summary(rec))
                {
                    out.push(Update::Agent(rec.clone()));
                }
                out.extend(alert.map(Update::Alert));
                out
            }
            ProbeMsg::Metrics(metrics) => {
                view.metrics = Some(metrics.clone());
                vec![Update::Metrics { id, metrics }]
            }
            ProbeMsg::Error { code, message } => vec![Update::ProbeError { id, code, message }],
            ProbeMsg::Hello { .. } | ProbeMsg::Heartbeat { .. } => Vec::new(),
        }
    }

    /// Mark agents of host `id` (in `session`, if given) as exited when
    /// `live(session, pane)` says their pane is gone.
    fn agents_gone(
        &mut self,
        id: &str,
        session: Option<&str>,
        live: impl Fn(&str, u32) -> bool,
        now_ms: u64,
    ) -> Vec<Update> {
        let gone: Vec<AgentKey> = self
            .tracker
            .records()
            .filter(|r| r.key.host_id == id)
            .filter(|r| session.is_none_or(|s| r.key.session == s))
            .filter(|r| r.state != AgentState::Exited)
            .filter(|r| !live(&r.key.session, r.key.pane_id))
            .map(|r| r.key.clone())
            .collect();
        gone.into_iter()
            .filter_map(|key| {
                self.tracker.pane_gone(&key, now_ms);
                self.tracker.get(&key).cloned().map(Update::Agent)
            })
            .collect()
    }
}
