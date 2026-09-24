//! Value types carried inside protocol messages.

use serde::{Deserialize, Serialize};

/// Lifecycle state of an agent running in a zellij pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Unknown,
    Working,
    NeedsInput,
    Done,
    Exited,
}

/// Where an agent event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    Hook,
    Scrape,
}

/// A pane inside a zellij session on one host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneRef {
    pub session: String,
    pub pane_id: u32,
}

/// A state observation for one agent pane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
    pub pane: PaneRef,
    pub agent: String,
    pub source: EventSource,
    pub state: AgentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub ts_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spool_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub name: String,
    pub exited: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: u32,
    pub tab_id: u32,
    pub tab_name: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub exited: bool,
}

/// Host metrics sample. Rates are bytes per second.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub cpu_pct: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
    pub net_rx_bps: u64,
    pub net_tx_bps: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load1: Option<f32>,
    pub ts_ms: u64,
}

/// Screen-scrape rules pushed from the app to the probe.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScrapeRules {
    #[serde(default)]
    pub agents: Vec<AgentRule>,
}

/// Regex rules for one agent kind. All patterns use `regex` crate syntax.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRule {
    pub name: String,
    /// Matched against pane title and pane command to identify the agent.
    #[serde(default)]
    pub title_patterns: Vec<String>,
    /// Matched against screen content to identify the agent.
    #[serde(default)]
    pub screen_patterns: Vec<String>,
    #[serde(default)]
    pub needs_input_patterns: Vec<String>,
    #[serde(default)]
    pub done_patterns: Vec<String>,
    /// Screen must be unchanged this long before idle states are inferred.
    pub stable_ms: u64,
}
