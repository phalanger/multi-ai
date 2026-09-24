use mai_core::tracker::{AgentKey, Tracker, TrackerConfig};
use mai_protocol::{AgentEvent, AgentState, EventSource, PaneRef};

use AgentState::{Done, Exited, NeedsInput, Working};
use EventSource::{Hook, Scrape};

const HOST: &str = "host-a";

fn ev(
    pane: u32,
    source: EventSource,
    state: AgentState,
    ts: u64,
) -> AgentEvent {
    AgentEvent {
        pane: PaneRef { session: "work".into(), pane_id: pane },
        agent: "claude".into(),
        source,
        state,
        message: None,
        ts_ms: ts,
        spool_offset: None,
    }
}

fn key(pane: u32) -> AgentKey {
    AgentKey { host_id: HOST.into(), session: "work".into(), pane_id: pane }
}

fn ev_offset(
    pane: u32,
    source: EventSource,
    state: AgentState,
    ts: u64,
    offset: u64,
) -> AgentEvent {
    let mut e = ev(pane, source, state, ts);
    e.spool_offset = Some(offset);
    e
}

struct Case {
    name: &'static str,
    steps: Vec<(EventSource, AgentState, u64, Option<AgentState>)>,
    final_state: AgentState,
}

#[test]
fn table() {
    let cases = vec![
        Case {
            name: "hook done alerts",
            steps: vec![(Hook, Working, 0, None), (Hook, Done, 1_000, Some(Done))],
            final_state: Done,
        },
        Case {
            name: "hook needs input alerts",
            steps: vec![
                (Hook, Working, 0, None),
                (Hook, NeedsInput, 1_000, Some(NeedsInput)),
            ],
            final_state: NeedsInput,
        },
        Case {
            name: "same state repeated does not alert",
            steps: vec![(Hook, Done, 0, Some(Done)), (Hook, Done, 1_000, None)],
            final_state: Done,
        },
        Case {
            name: "dedupe within window",
            steps: vec![
                (Hook, Done, 0, Some(Done)),
                (Hook, Working, 1_000, None),
                (Hook, Done, 2_000, None),
            ],
            final_state: Done,
        },
        Case {
            name: "alerts again after dedupe window",
            steps: vec![
                (Hook, Done, 0, Some(Done)),
                (Hook, Working, 1_000, None),
                (Hook, Done, 31_000, Some(Done)),
            ],
            final_state: Done,
        },
        Case {
            name: "scrape ignored while hook authoritative",
            steps: vec![(Hook, Working, 0, None), (Scrape, Done, 1_000, None)],
            final_state: Working,
        },
        Case {
            name: "scrape may flip done to working",
            steps: vec![
                (Hook, Done, 0, Some(Done)),
                (Scrape, Working, 1_000, None),
            ],
            final_state: Working,
        },
        Case {
            name: "scrape accepted after hook window",
            steps: vec![
                (Hook, Working, 0, None),
                (Scrape, Done, 600_000, Some(Done)),
            ],
            final_state: Done,
        },
        Case {
            name: "scrape only pane",
            steps: vec![
                (Scrape, Working, 0, None),
                (Scrape, NeedsInput, 1_000, Some(NeedsInput)),
            ],
            final_state: NeedsInput,
        },
        Case {
            name: "exited does not alert",
            steps: vec![(Hook, Exited, 0, None)],
            final_state: Exited,
        },
    ];
    for c in cases {
        let mut t = Tracker::new(TrackerConfig::default());
        for (i, (src, st, ts, want)) in c.steps.iter().enumerate() {
            let got = t.apply(HOST, &ev(1, *src, *st, *ts)).map(|a| a.state);
            assert_eq!(got, *want, "case '{}' step {i}", c.name);
        }
        let rec = t.get(&key(1)).expect("record");
        assert_eq!(rec.state, c.final_state, "case '{}' final", c.name);
    }
}

#[test]
fn alert_carries_key_agent_and_message() {
    let mut t = Tracker::new(TrackerConfig::default());
    let mut e = ev(7, Hook, NeedsInput, 5);
    e.message = Some("allow bash?".into());
    let a = t.apply(HOST, &e).expect("alert");
    assert_eq!(a.key, key(7));
    assert_eq!(a.agent, "claude");
    assert_eq!(a.message.as_deref(), Some("allow bash?"));
}

#[test]
fn pending_orders_needs_input_first_then_by_time() {
    let mut t = Tracker::new(TrackerConfig::default());
    t.apply(HOST, &ev(1, Hook, Done, 100));
    t.apply(HOST, &ev(2, Hook, NeedsInput, 300));
    t.apply(HOST, &ev(3, Hook, NeedsInput, 200));
    t.apply(HOST, &ev(4, Hook, Working, 50));
    let order: Vec<u32> = t.pending().iter().map(|r| r.key.pane_id).collect();
    assert_eq!(order, vec![3, 2, 1]);
}

#[test]
fn acknowledge_removes_from_pending() {
    let mut t = Tracker::new(TrackerConfig::default());
    t.apply(HOST, &ev(1, Hook, Done, 0));
    assert!(t.acknowledge(&key(1)));
    assert!(t.pending().is_empty());
    assert!(!t.acknowledge(&key(99)));
}

#[test]
fn deduped_state_is_still_pending() {
    let mut t = Tracker::new(TrackerConfig::default());
    t.apply(HOST, &ev(1, Hook, Done, 0));
    t.apply(HOST, &ev(1, Hook, Working, 1_000));
    assert!(t.apply(HOST, &ev(1, Hook, Done, 2_000)).is_none());
    assert_eq!(t.pending().len(), 1);
}

#[test]
fn pane_gone_marks_exited_and_clears_pending() {
    let mut t = Tracker::new(TrackerConfig::default());
    t.apply(HOST, &ev(1, Hook, NeedsInput, 0));
    t.pane_gone(&key(1), 10);
    assert_eq!(t.get(&key(1)).unwrap().state, Exited);
    assert!(t.pending().is_empty());
}

#[test]
fn hosts_are_isolated() {
    let mut t = Tracker::new(TrackerConfig::default());
    t.apply("host-a", &ev(1, Hook, Done, 0));
    t.apply("host-b", &ev(1, Hook, Working, 0));
    assert_eq!(t.get(&key(1)).unwrap().state, Done);
}

#[test]
fn replayed_hook_events_are_ignored() {
    let mut t = Tracker::new(TrackerConfig::default());
    assert!(t.apply(HOST, &ev_offset(1, Hook, Working, 1, 1)).is_none());
    let alert = t.apply(HOST, &ev_offset(1, Hook, Done, 2, 2));
    assert_eq!(alert.map(|a| a.state), Some(Done));
    assert!(t.acknowledge(&key(1)));

    assert!(t.apply(HOST, &ev_offset(1, Hook, Working, 1, 1)).is_none());
    assert!(t.apply(HOST, &ev_offset(1, Hook, Done, 2, 2)).is_none());

    let rec = t.get(&key(1)).expect("record");
    assert_eq!(rec.state, Done);
    assert!(rec.acknowledged);
    assert_eq!(rec.since_ms, 2);
    assert!(t.pending().is_empty());
}

#[test]
fn older_hook_without_offset_is_ignored() {
    let mut t = Tracker::new(TrackerConfig::default());
    let alert = t.apply(HOST, &ev(1, Hook, Done, 5_000));
    assert_eq!(alert.map(|a| a.state), Some(Done));

    assert!(t.apply(HOST, &ev(1, Hook, Working, 1_000)).is_none());
    assert_eq!(t.get(&key(1)).unwrap().state, Done);
}

#[test]
fn dedupe_boundary_is_exclusive() {
    let mut t = Tracker::new(TrackerConfig::default());
    let a1 = t.apply(HOST, &ev(1, Hook, Done, 0));
    assert_eq!(a1.map(|a| a.state), Some(Done));
    assert!(t.apply(HOST, &ev(1, Hook, Working, 1_000)).is_none());
    let a2 = t.apply(HOST, &ev(1, Hook, Done, 30_000));
    assert_eq!(a2.map(|a| a.state), Some(Done));
}

#[test]
fn hook_authority_boundary_is_exclusive() {
    let mut t = Tracker::new(TrackerConfig::default());
    assert!(t.apply(HOST, &ev(1, Hook, Working, 0)).is_none());
    assert!(t.apply(HOST, &ev(1, Scrape, Done, 599_999)).is_none());
    let a = t.apply(HOST, &ev(1, Scrape, Done, 600_000));
    assert_eq!(a.map(|a| a.state), Some(Done));
}

#[test]
fn pending_tie_break_is_deterministic() {
    let mut t = Tracker::new(TrackerConfig::default());
    t.apply("host-b", &ev(1, Hook, Done, 100));
    t.apply("host-a", &ev(1, Hook, Done, 100));
    let order: Vec<&str> =
        t.pending().iter().map(|r| r.key.host_id.as_str()).collect();
    assert_eq!(order, vec!["host-a", "host-b"]);
}
