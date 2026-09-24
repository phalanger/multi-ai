use mai_probe::scrape::{CompiledRules, ScrapeTracker};
use mai_protocol::{AgentRule, AgentState, PaneRef, ScrapeRules};

fn rules() -> CompiledRules {
    CompiledRules::compile(&ScrapeRules {
        agents: vec![AgentRule {
            name: "fake".into(),
            title_patterns: vec!["^fakeagent".into()],
            screen_patterns: vec![r"FAKE AGENT v\d".into()],
            needs_input_patterns: vec![r"(?m)^Allow\? \[y/n\]".into()],
            done_patterns: vec![r"(?m)^> $".into()],
            ignore_patterns: vec![],
            stable_ms: 5_000,
        }],
    })
    .unwrap()
}

fn pane() -> PaneRef {
    PaneRef {
        session: "work".into(),
        pane_id: 1,
    }
}

const ASK: &str = "FAKE AGENT v1\nrun rm?\nAllow? [y/n]\n";
const IDLE: &str = "FAKE AGENT v1\nall done\n> \n";

#[test]
fn identify_by_title_command_or_screen() {
    let r = rules();
    assert_eq!(r.identify("fakeagent - x", None, ""), Some("fake"));
    assert_eq!(r.identify("bash", Some("fakeagent --x"), ""), Some("fake"));
    assert_eq!(
        r.identify("bash", None, "FAKE AGENT v2 ready"),
        Some("fake")
    );
    assert_eq!(r.identify("bash", None, "$ ls"), None);
}

#[test]
fn compile_rejects_bad_regex() {
    let bad = ScrapeRules {
        agents: vec![AgentRule {
            name: "x".into(),
            title_patterns: vec!["(".into()],
            screen_patterns: vec![],
            needs_input_patterns: vec![],
            done_patterns: vec![],
            ignore_patterns: vec![],
            stable_ms: 1,
        }],
    };
    assert!(CompiledRules::compile(&bad).is_err());
}

#[test]
fn first_observation_is_baseline_only() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    assert_eq!(t.observe(&r, "fake", &pane(), IDLE, 0), None);
}

#[test]
fn change_emits_working_once() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "fake", &pane(), "a", 0);
    let got = t.observe(&r, "fake", &pane(), "b", 1_000);
    assert_eq!(got, Some(AgentState::Working));
    assert_eq!(t.observe(&r, "fake", &pane(), "c", 2_000), None);
}

#[test]
fn stable_prompt_becomes_needs_input_after_stable_ms() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "fake", &pane(), "a", 0);
    let got = t.observe(&r, "fake", &pane(), ASK, 1_000);
    assert_eq!(got, Some(AgentState::Working));
    assert_eq!(t.observe(&r, "fake", &pane(), ASK, 3_000), None);
    let got = t.observe(&r, "fake", &pane(), ASK, 6_000);
    assert_eq!(got, Some(AgentState::NeedsInput));
    assert_eq!(t.observe(&r, "fake", &pane(), ASK, 7_000), None);
}

#[test]
fn stable_idle_prompt_becomes_done() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "fake", &pane(), IDLE, 0);
    let got = t.observe(&r, "fake", &pane(), IDLE, 5_000);
    assert_eq!(got, Some(AgentState::Done));
}

#[test]
fn stable_without_match_emits_nothing() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "fake", &pane(), "thinking", 0);
    assert_eq!(t.observe(&r, "fake", &pane(), "thinking", 9_000), None);
}

#[test]
fn working_re_emitted_after_stable_unmatched_period() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "fake", &pane(), "a", 0);
    let got = t.observe(&r, "fake", &pane(), "b", 1_000);
    assert_eq!(got, Some(AgentState::Working));
    assert_eq!(t.observe(&r, "fake", &pane(), "b", 7_000), None);
    let got = t.observe(&r, "fake", &pane(), "c", 8_000);
    assert_eq!(got, Some(AgentState::Working));
}

#[test]
fn unknown_agent_returns_none() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "nope", &pane(), "a", 0);
    assert_eq!(t.observe(&r, "nope", &pane(), "b", 1_000), None);
}

#[test]
fn forget_resets_baseline() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "fake", &pane(), "a", 0);
    t.forget(&pane());
    assert_eq!(t.observe(&r, "fake", &pane(), "b", 1_000), None);
}
