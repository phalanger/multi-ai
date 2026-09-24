//! Maps raw hook payloads (as captured in the spool) to agent states.
//!
//! Event names and fields follow the real payloads captured in the spike
//! (docs/superpowers/spike-data/*.jsonl).

use mai_protocol::AgentState;
use serde_json::Value;

/// Longest message forwarded to the app, in characters.
const MAX_MESSAGE_CHARS: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapped {
    pub state: AgentState,
    pub message: Option<String>,
}

fn mapped(state: AgentState, message: Option<String>) -> Option<Mapped> {
    Some(Mapped { state, message })
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// First non-empty line, cut to `MAX_MESSAGE_CHARS` characters.
fn short(text: Option<&str>) -> Option<String> {
    let line = text?.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(line.chars().take(MAX_MESSAGE_CHARS).collect())
}

fn map_claude(p: &Value) -> Option<Mapped> {
    use AgentState::*;
    match str_field(p, "hook_event_name")? {
        "SessionStart" => mapped(Unknown, None),
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => mapped(Working, None),
        "Notification" => match str_field(p, "notification_type")? {
            "permission_prompt" => mapped(NeedsInput, short(str_field(p, "message"))),
            "idle_prompt" => mapped(Done, None),
            _ => None,
        },
        "Stop" => mapped(Done, short(str_field(p, "last_assistant_message"))),
        "SessionEnd" => mapped(Exited, None),
        _ => None,
    }
}

fn map_codex(p: &Value) -> Option<Mapped> {
    use AgentState::*;
    match str_field(p, "hook_event_name")? {
        "SessionStart" => mapped(Unknown, None),
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => mapped(Working, None),
        "PermissionRequest" => {
            let input = p.get("tool_input").unwrap_or(&Value::Null);
            let msg = str_field(input, "description").or(str_field(p, "tool_name"));
            mapped(NeedsInput, short(msg))
        }
        "Stop" => mapped(Done, short(str_field(p, "last_assistant_message"))),
        "Interrupt" => mapped(Done, None),
        "SessionEnd" => mapped(Exited, None),
        _ => None,
    }
}

/// Payload written by `mai-probe emit`: `{"state": "...", "message": ...}`.
fn map_generic(p: &Value) -> Option<Mapped> {
    let state: AgentState = serde_json::from_value(p.get("state")?.clone()).ok()?;
    mapped(state, short(str_field(p, "message")))
}

/// State implied by one hook payload; `None` for events that do not
/// change state (e.g. compaction, subagents).
pub fn map_hook(agent: &str, payload: &Value) -> Option<Mapped> {
    match agent {
        "claude" => map_claude(payload),
        "codex" => map_codex(payload),
        _ => map_generic(payload),
    }
}
