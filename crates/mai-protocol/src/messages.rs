//! Top-level messages in each direction.

use serde::{Deserialize, Serialize};

use crate::types::{
    AgentEvent, Metrics, PaneInfo, ScrapeRules, SessionInfo,
};

/// Probe -> app (probe stdout).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProbeMsg {
    Hello {
        protocol_version: u32,
        probe_version: String,
        os: String,
        arch: String,
        zellij_path: Option<String>,
        zellij_version: Option<String>,
    },
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    Panes {
        session: String,
        panes: Vec<PaneInfo>,
    },
    AgentEvent(AgentEvent),
    Metrics(Metrics),
    Heartbeat {
        ts_ms: u64,
    },
    Error {
        code: String,
        message: String,
    },
}

/// App -> probe (probe stdin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMsg {
    /// Acknowledges every event up to and including `spool_offset`, the
    /// opaque, strictly increasing cursor the probe assigns per host
    /// across all of its spool files.
    Ack {
        spool_offset: u64,
    },
    SendText {
        session: String,
        pane_id: u32,
        text: String,
    },
    Focus {
        session: String,
        pane_id: u32,
        tab_id: u32,
    },
    SetInterval {
        pane_poll_ms: u64,
        scrape_ms: u64,
        metrics_ms: u64,
    },
    SetRules {
        rules: ScrapeRules,
    },
}
