use mai_probe::hookmap::{Mapped, map_hook};
use mai_protocol::AgentState::{self, Done, Exited, NeedsInput, Unknown, Working};
use serde_json::{Value, json};

/// Real payloads captured in the spike: (agent, payload) per hook call.
/// Lines from Codex `notify` (argv[0] == "codex-notify") are skipped.
fn captured(file: &str) -> Vec<(String, Value)> {
    let path = format!("{}/tests/fixtures/hooks/{file}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(path)
        .expect("fixture")
        .lines()
        .filter_map(|line| {
            let v: Value = serde_json::from_str(line).expect("fixture line");
            let agent = match v["argv"][0].as_str()? {
                "claude" => "claude",
                "codex-hook" => "codex",
                _ => return None,
            };
            let payload = serde_json::from_str(v["stdin"].as_str()?).expect("payload");
            Some((agent.to_owned(), payload))
        })
        .collect()
}

fn expected(agent: &str, p: &Value) -> Option<AgentState> {
    let event = p["hook_event_name"].as_str().unwrap();
    match (agent, event) {
        (_, "SessionStart") => Some(Unknown),
        (_, "UserPromptSubmit" | "PreToolUse" | "PostToolUse") => Some(Working),
        ("claude", "Notification") => match p["notification_type"].as_str() {
            Some("permission_prompt") => Some(NeedsInput),
            Some("idle_prompt") => Some(Done),
            _ => None,
        },
        ("codex", "PermissionRequest") => Some(NeedsInput),
        (_, "Stop") | ("codex", "Interrupt") => Some(Done),
        (_, "SessionEnd") => Some(Exited),
        _ => None,
    }
}

#[test]
fn every_captured_claude_event_maps_as_expected() {
    let events = captured("claude-hooks.jsonl");
    assert!(events.len() >= 8, "fixture too small: {}", events.len());
    for (agent, p) in &events {
        let got = map_hook(agent, p).map(|m| m.state);
        assert_eq!(got, expected(agent, p), "{}", p["hook_event_name"]);
    }
}

#[test]
fn every_captured_codex_event_maps_as_expected() {
    let events = captured("codex-events.jsonl");
    assert!(events.len() >= 7, "fixture too small: {}", events.len());
    for (agent, p) in &events {
        let got = map_hook(agent, p).map(|m| m.state);
        assert_eq!(got, expected(agent, p), "{}", p["hook_event_name"]);
    }
}

#[test]
fn captured_samples_cover_every_state() {
    let mut seen = Vec::new();
    for file in ["claude-hooks.jsonl", "codex-events.jsonl"] {
        for (agent, p) in captured(file) {
            if let Some(m) = map_hook(&agent, &p) {
                seen.push((agent, m.state));
            }
        }
    }
    for agent in ["claude", "codex"] {
        for state in [Unknown, Working, NeedsInput, Done, Exited] {
            assert!(
                seen.iter().any(|(a, s)| a == agent && *s == state),
                "{agent} never reached {state:?}"
            );
        }
    }
}

#[test]
fn claude_permission_message_is_forwarded() {
    let p = json!({
        "hook_event_name": "Notification",
        "notification_type": "permission_prompt",
        "message": "Claude needs your permission"
    });
    assert_eq!(
        map_hook("claude", &p),
        Some(Mapped {
            state: NeedsInput,
            message: Some("Claude needs your permission".into())
        })
    );
}

#[test]
fn codex_permission_prefers_description() {
    let p = json!({
        "hook_event_name": "PermissionRequest",
        "tool_name": "Bash",
        "tool_input": {"command": "rm x", "description": "Allow deleting x?"}
    });
    assert_eq!(
        map_hook("codex", &p).unwrap().message.as_deref(),
        Some("Allow deleting x?")
    );
    let p = json!({"hook_event_name": "PermissionRequest", "tool_name": "Bash"});
    assert_eq!(
        map_hook("codex", &p).unwrap().message.as_deref(),
        Some("Bash")
    );
}

#[test]
fn stop_message_is_first_line_truncated() {
    let long = format!("\n  {}\nsecond", "x".repeat(300));
    let p = json!({"hook_event_name": "Stop", "last_assistant_message": long});
    let msg = map_hook("claude", &p).unwrap().message.unwrap();
    assert_eq!(msg, "x".repeat(120));
}

#[test]
fn generic_emit_payload() {
    let p = json!({"state": "needs_input", "message": "pick a branch"});
    let m = map_hook("cmagent", &p).unwrap();
    assert_eq!(m.state, NeedsInput);
    assert_eq!(m.message.as_deref(), Some("pick a branch"));
    assert_eq!(map_hook("cmagent", &json!({"state": "bogus"})), None);
}

#[test]
fn unknown_events_and_types_map_to_none() {
    assert_eq!(
        map_hook("claude", &json!({"hook_event_name": "PreCompact"})),
        None
    );
    let p = json!({"hook_event_name": "Notification", "notification_type": "auth_success"});
    assert_eq!(map_hook("claude", &p), None);
    assert_eq!(
        map_hook("codex", &json!({"hook_event_name": "SubagentStop"})),
        None
    );
    assert_eq!(map_hook("claude", &json!({})), None);
}
