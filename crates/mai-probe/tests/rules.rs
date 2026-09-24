use mai_probe::rules::{default_rules, parse_rules};
use mai_probe::scrape::{CompiledRules, ScrapeTracker};
use mai_protocol::{AgentRule, AgentState, PaneRef, ScrapeRules};

const CMD_TITLE: &str = "C:\\WINDOWS\\system32\\cmd.exe";

fn screen(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/screens/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).expect("fixture")
}

fn compiled() -> CompiledRules {
    CompiledRules::compile(&default_rules()).expect("default rules compile")
}

fn pane() -> PaneRef {
    PaneRef {
        session: "s".into(),
        pane_id: 1,
    }
}

/// Feed the same screen twice, 5 s apart, and return the inferred state.
fn stable_state(rules: &CompiledRules, agent: &str, text: &str) -> Option<AgentState> {
    let mut t = ScrapeTracker::default();
    t.observe(rules, agent, &pane(), text, 0);
    t.observe(rules, agent, &pane(), text, 5_000)
}

#[test]
fn default_rules_identify_every_sample_by_screen() {
    let r = compiled();
    for name in [
        "claude-working",
        "claude-done",
        "claude-idle",
        "claude-needs-input",
    ] {
        assert_eq!(
            r.identify(CMD_TITLE, None, &screen(name)),
            Some("claude"),
            "{name}"
        );
    }
    for name in [
        "codex-working",
        "codex-done",
        "codex-idle",
        "codex-needs-input",
    ] {
        assert_eq!(
            r.identify(CMD_TITLE, None, &screen(name)),
            Some("codex"),
            "{name}"
        );
    }
}

#[test]
fn default_rules_identify_by_title_or_command() {
    let r = compiled();
    assert_eq!(r.identify_by_title("claude", None), Some("claude"));
    assert_eq!(r.identify_by_title("✳ Claude Code", None), Some("claude"));
    let running = "claude.exe --dangerously-skip-permissions -c";
    assert_eq!(
        r.identify_by_title(CMD_TITLE, Some(running)),
        Some("claude")
    );
    let cmd = "C:\\Users\\x\\.local\\bin\\claude.EXE --dangerously-skip-permissions -c";
    assert_eq!(r.identify_by_title(CMD_TITLE, Some(cmd)), Some("claude"));
    let codex = "C:\\nvm4w\\nodejs\\\\node.exe C:\\nvm4w\\nodejs\\\\node_modules\\@openai\\codex\\bin\\codex.js resume";
    assert_eq!(r.identify_by_title(codex, Some(codex)), Some("codex"));
    assert_eq!(r.identify_by_title("claude-playground", None), None);
    assert_eq!(r.identify_by_title(CMD_TITLE, None), None);
}

#[test]
fn default_rules_classify_stable_samples() {
    let r = compiled();
    let cases = [
        ("claude", "claude-needs-input", Some(AgentState::NeedsInput)),
        ("claude", "claude-done", Some(AgentState::Done)),
        ("claude", "claude-idle", Some(AgentState::Done)),
        ("claude", "claude-working", None),
        ("codex", "codex-needs-input", Some(AgentState::NeedsInput)),
        ("codex", "codex-done", Some(AgentState::Done)),
        ("codex", "codex-idle", Some(AgentState::Done)),
        ("codex", "codex-working", None),
    ];
    for (agent, name, want) in cases {
        assert_eq!(stable_state(&r, agent, &screen(name)), want, "{name}");
    }
}

#[test]
fn claude_status_clock_change_is_ignored() {
    let r = compiled();
    let a = screen("claude-idle");
    assert!(a.contains("resets in 3h 21m"));
    let b = a.replace("resets in 3h 21m", "resets in 3h 20m");
    let mut t = ScrapeTracker::default();
    t.observe(&r, "claude", &pane(), &a, 0);
    assert_eq!(
        t.observe(&r, "claude", &pane(), &b, 5_000),
        Some(AgentState::Done)
    );
}

#[test]
fn codex_braille_animation_is_ignored() {
    let r = compiled();
    let a = screen("codex-done");
    let b: String = a
        .chars()
        .map(|c| {
            if ('\u{2800}'..='\u{28FF}').contains(&c) {
                '\u{2801}'
            } else {
                c
            }
        })
        .collect();
    assert_ne!(a, b);
    let mut t = ScrapeTracker::default();
    t.observe(&r, "codex", &pane(), &a, 0);
    assert_eq!(
        t.observe(&r, "codex", &pane(), &b, 5_000),
        Some(AgentState::Done)
    );
}

#[test]
fn parse_rules_rejects_bad_toml() {
    assert!(parse_rules("agents = 3").is_err());
}

#[test]
fn rule_error_names_agent_and_pattern() {
    let bad = ScrapeRules {
        agents: vec![AgentRule {
            name: "broken".into(),
            title_patterns: vec![],
            screen_patterns: vec![],
            needs_input_patterns: vec![],
            done_patterns: vec![],
            ignore_patterns: vec!["(".into()],
            stable_ms: 1,
        }],
    };
    let err = CompiledRules::compile(&bad).err().expect("must fail");
    let msg = err.to_string();
    assert!(msg.contains("broken") && msg.contains("'('"), "{msg}");
}
