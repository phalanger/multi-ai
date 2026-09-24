//! Wire protocol shared by mai-probe and the multi-ai app.
//!
//! Transport is JSON Lines: one JSON object per line, tagged by `type`.

mod codec;
mod messages;
mod types;

pub use codec::{decode_line, encode_line};
pub use messages::{AppMsg, ProbeMsg};
pub use types::{
    AgentEvent, AgentRule, AgentState, EventSource, Metrics, PaneInfo,
    PaneRef, ScrapeRules, SessionInfo,
};

/// Bumped on any incompatible change to message shapes.
pub const PROTOCOL_VERSION: u32 = 1;
