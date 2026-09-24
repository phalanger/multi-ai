//! Per-agent state tracking: merges hook and scrape events and decides
//! when the user must be alerted.

use std::collections::HashMap;

use mai_protocol::{AgentEvent, AgentState, EventSource};

/// Identifies one agent: a pane in a zellij session on a host.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentKey {
    pub host_id: String,
    pub session: String,
    pub pane_id: u32,
}

impl AgentKey {
    pub fn from_event(host_id: &str, ev: &AgentEvent) -> Self {
        Self {
            host_id: host_id.to_owned(),
            session: ev.pane.session.clone(),
            pane_id: ev.pane.pane_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackerConfig {
    /// While a hook event is newer than this, scrape events may only
    /// move an idle agent back to Working.
    pub hook_authority_ms: u64,
    /// Re-entering the same alerting state within this window is silent.
    pub alert_dedupe_ms: u64,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self { hook_authority_ms: 600_000, alert_dedupe_ms: 30_000 }
    }
}

/// Emitted when the user should be notified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub key: AgentKey,
    pub agent: String,
    pub state: AgentState,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRecord {
    pub key: AgentKey,
    pub agent: String,
    pub state: AgentState,
    pub message: Option<String>,
    /// False while an alerting state has not been seen by the user.
    pub acknowledged: bool,
    /// Timestamp at which `state` was entered.
    pub since_ms: u64,
    last_hook_ms: Option<u64>,
    last_hook_offset: Option<u64>,
    last_alert: Option<(AgentState, u64)>,
}

#[derive(Debug)]
pub struct Tracker {
    cfg: TrackerConfig,
    agents: HashMap<AgentKey, AgentRecord>,
}

fn is_alerting(s: AgentState) -> bool {
    matches!(s, AgentState::NeedsInput | AgentState::Done)
}

fn scrape_may_override(from: AgentState, to: AgentState) -> bool {
    is_alerting(from) && to == AgentState::Working
}

impl Tracker {
    pub fn new(cfg: TrackerConfig) -> Self {
        Self { cfg, agents: HashMap::new() }
    }

    /// Apply one event; returns an alert when the user should be notified.
    pub fn apply(&mut self, host_id: &str, ev: &AgentEvent) -> Option<Alert> {
        let key = AgentKey::from_event(host_id, ev);
        let cfg = self.cfg;
        let rec = self.agents.entry(key.clone()).or_insert_with(|| {
            AgentRecord {
                key: key.clone(),
                agent: ev.agent.clone(),
                state: AgentState::Unknown,
                message: None,
                acknowledged: true,
                since_ms: ev.ts_ms,
                last_hook_ms: None,
                last_hook_offset: None,
                last_alert: None,
            }
        });
        match ev.source {
            EventSource::Hook => {
                if let Some(o) = ev.spool_offset {
                    if rec.last_hook_offset.is_some_and(|prev| o <= prev) {
                        return None;
                    }
                } else if rec.last_hook_ms.is_some_and(|h| ev.ts_ms < h) {
                    return None;
                }
                rec.last_hook_ms = Some(ev.ts_ms);
                if let Some(o) = ev.spool_offset {
                    rec.last_hook_offset = Some(o);
                }
            }
            EventSource::Scrape => {
                let authoritative = rec.last_hook_ms.is_some_and(|h| {
                    ev.ts_ms.saturating_sub(h) < cfg.hook_authority_ms
                });
                if authoritative && !scrape_may_override(rec.state, ev.state) {
                    return None;
                }
            }
        }
        if rec.state == ev.state {
            return None;
        }
        rec.agent = ev.agent.clone();
        rec.state = ev.state;
        rec.message = ev.message.clone();
        rec.since_ms = ev.ts_ms;
        if !is_alerting(ev.state) {
            rec.acknowledged = true;
            return None;
        }
        rec.acknowledged = false;
        if let Some((s, t)) = rec.last_alert
            && s == ev.state
            && ev.ts_ms.saturating_sub(t) < cfg.alert_dedupe_ms
        {
            return None;
        }
        rec.last_alert = Some((ev.state, ev.ts_ms));
        Some(Alert {
            key,
            agent: rec.agent.clone(),
            state: ev.state,
            message: rec.message.clone(),
        })
    }

    /// The pane disappeared or exited.
    pub fn pane_gone(&mut self, key: &AgentKey, now_ms: u64) {
        if let Some(rec) = self.agents.get_mut(key) {
            rec.state = AgentState::Exited;
            rec.since_ms = now_ms;
            rec.acknowledged = true;
        }
    }

    /// Mark the current alert as seen. Returns false for unknown keys.
    pub fn acknowledge(&mut self, key: &AgentKey) -> bool {
        match self.agents.get_mut(key) {
            Some(rec) => {
                rec.acknowledged = true;
                true
            }
            None => false,
        }
    }

    pub fn get(&self, key: &AgentKey) -> Option<&AgentRecord> {
        self.agents.get(key)
    }

    /// Unacknowledged alerting agents: NeedsInput first, then Done,
    /// each group oldest first.
    pub fn pending(&self) -> Vec<&AgentRecord> {
        let mut v: Vec<&AgentRecord> = self
            .agents
            .values()
            .filter(|r| is_alerting(r.state) && !r.acknowledged)
            .collect();
        v.sort_by_key(|r| {
            let rank = u8::from(r.state != AgentState::NeedsInput);
            (rank, r.since_ms, r.key.pane_id)
        });
        v
    }
}
