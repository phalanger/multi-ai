use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use mai_probe::rules::default_rules;
use mai_probe::serve::{CLEANUP_EVERY_MS, Intervals, Server};
use mai_probe::spool::{Spool, SpoolRecord};
use mai_probe::zellij::{Zellij, ZellijError};
use mai_protocol::{
    AgentEvent, AgentRule, AgentState, AppMsg, EventSource, PaneInfo, ProbeMsg, ScrapeRules,
    SessionInfo, encode_line,
};
use serde_json::json;

#[derive(Default)]
struct FakeState {
    sessions: Vec<SessionInfo>,
    panes: HashMap<String, Vec<PaneInfo>>,
    screens: HashMap<u32, String>,
    fail_panes: bool,
    calls: Vec<String>,
}

#[derive(Clone, Default)]
struct Fake(Rc<RefCell<FakeState>>);

impl Zellij for Fake {
    fn sessions(&self) -> Result<Vec<SessionInfo>, ZellijError> {
        Ok(self.0.borrow().sessions.clone())
    }
    fn panes(&self, session: &str) -> Result<Vec<PaneInfo>, ZellijError> {
        let s = self.0.borrow();
        if s.fail_panes {
            return Err(ZellijError("boom".into()));
        }
        Ok(s.panes.get(session).cloned().unwrap_or_default())
    }
    fn dump_screen(&self, _session: &str, pane_id: u32) -> Result<String, ZellijError> {
        Ok(self
            .0
            .borrow()
            .screens
            .get(&pane_id)
            .cloned()
            .unwrap_or_default())
    }
    fn paste(&self, session: &str, pane_id: u32, text: &str) -> Result<(), ZellijError> {
        self.0
            .borrow_mut()
            .calls
            .push(format!("paste {session} {pane_id} {text}"));
        Ok(())
    }
    fn focus(&self, session: &str, tab_id: u32, pane_id: u32) -> Result<(), ZellijError> {
        self.0
            .borrow_mut()
            .calls
            .push(format!("focus {session} {tab_id} {pane_id}"));
        Ok(())
    }
}

fn pane(id: u32, title: &str) -> PaneInfo {
    PaneInfo {
        id,
        tab_id: 0,
        tab_name: "t".into(),
        title: title.into(),
        command: None,
        exited: false,
    }
}

fn session(name: &str) -> SessionInfo {
    SessionInfo {
        name: name.into(),
        exited: false,
    }
}

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/screens/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap()
}

fn stop_record(ts_ms: u64, pane_id: u32) -> SpoolRecord {
    SpoolRecord {
        ts_ms,
        agent: "claude".into(),
        session: "work".into(),
        pane_id,
        payload: json!({"hook_event_name": "Stop", "last_assistant_message": "all done"}),
    }
}

fn events(msgs: &[ProbeMsg]) -> Vec<&AgentEvent> {
    msgs.iter()
        .filter_map(|m| match m {
            ProbeMsg::AgentEvent(e) => Some(e),
            _ => None,
        })
        .collect()
}

fn error_codes(msgs: &[ProbeMsg]) -> Vec<&str> {
    msgs.iter()
        .filter_map(|m| match m {
            ProbeMsg::Error { code, .. } => Some(code.as_str()),
            _ => None,
        })
        .collect()
}

fn server(fake: &Fake, dir: &std::path::Path) -> Server<Fake> {
    Server::new(Some(fake.clone()), Spool::new(dir), &default_rules()).unwrap()
}

#[test]
fn hook_records_become_events_once() {
    let dir = tempfile::tempdir().unwrap();
    Spool::new(dir.path())
        .append(&stop_record(1_000, 7))
        .unwrap();
    let mut s = server(&Fake::default(), dir.path());
    let out = s.tick(0);
    let ev = events(&out);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].state, AgentState::Done);
    assert_eq!(ev[0].source, EventSource::Hook);
    assert_eq!(ev[0].pane.pane_id, 7);
    assert_eq!(ev[0].message.as_deref(), Some("all done"));
    assert!(ev[0].spool_offset.is_some());
    assert!(events(&s.tick(10)).is_empty());
}

#[test]
fn restart_resumes_after_ack() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path());
    spool.append(&stop_record(1_000, 1)).unwrap();
    let mut first = server(&Fake::default(), dir.path());
    let cursor = events(&first.tick(0))[0].spool_offset.unwrap();
    assert!(
        first
            .handle(AppMsg::Ack {
                spool_offset: cursor
            })
            .is_empty()
    );

    spool.append(&stop_record(2_000, 2)).unwrap();
    let mut second = server(&Fake::default(), dir.path());
    let out = second.tick(0);
    let panes: Vec<u32> = events(&out).iter().map(|e| e.pane.pane_id).collect();
    assert_eq!(panes, vec![2]);
}

#[test]
fn sessions_and_panes_are_sent_only_on_change() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![
            session("work"),
            SessionInfo {
                name: "old".into(),
                exited: true,
            },
        ];
        st.panes.insert("work".into(), vec![pane(1, "bash")]);
        st.panes.insert("old".into(), vec![pane(9, "never polled")]);
    }
    let mut s = server(&fake, dir.path());
    let out = s.tick(0);
    assert!(matches!(&out[0], ProbeMsg::Sessions { sessions } if sessions.len() == 2));
    let panes_msgs: Vec<&str> = out
        .iter()
        .filter_map(|m| match m {
            ProbeMsg::Panes { session, .. } => Some(session.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(panes_msgs, vec!["work"]);

    let out = s.tick(2_000);
    assert!(
        !out.iter()
            .any(|m| matches!(m, ProbeMsg::Sessions { .. } | ProbeMsg::Panes { .. }))
    );

    fake.0
        .borrow_mut()
        .panes
        .insert("work".into(), vec![pane(1, "bash"), pane(2, "vim")]);
    let out = s.tick(4_000);
    assert!(
        out.iter()
            .any(|m| matches!(m, ProbeMsg::Panes { panes, .. } if panes.len() == 2))
    );
}

#[test]
fn scrape_identifies_agent_then_infers_state() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert(
            "work".into(),
            vec![pane(4, "C:\\WINDOWS\\system32\\cmd.exe")],
        );
        st.screens.insert(4, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let ev: Vec<(AgentState, EventSource)> = events(&s.tick(0))
        .iter()
        .map(|e| (e.state, e.source))
        .collect();
    assert_eq!(ev, vec![(AgentState::Unknown, EventSource::Scrape)]);
    assert!(events(&s.tick(3_000)).is_empty());
    let out = s.tick(6_000);
    let ev = events(&out);
    assert_eq!(ev.len(), 1);
    assert_eq!(
        (ev[0].agent.as_str(), ev[0].state),
        ("claude", AgentState::Done)
    );
}

#[test]
fn hook_known_pane_is_not_announced_again() {
    let dir = tempfile::tempdir().unwrap();
    Spool::new(dir.path()).append(&stop_record(1, 4)).unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "x")]);
        st.screens.insert(4, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let ev = events(&s.tick(0))
        .iter()
        .map(|e| (e.state, e.source))
        .collect::<Vec<_>>();
    assert_eq!(ev, vec![(AgentState::Done, EventSource::Hook)]);
}

#[test]
fn vanished_pane_is_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "claude")]);
    }
    let mut s = server(&fake, dir.path());
    assert_eq!(events(&s.tick(0)).len(), 1);
    fake.0.borrow_mut().panes.insert("work".into(), vec![]);
    s.tick(3_000);
    fake.0
        .borrow_mut()
        .panes
        .insert("work".into(), vec![pane(4, "claude")]);
    let ev: Vec<AgentState> = events(&s.tick(6_000)).iter().map(|e| e.state).collect();
    assert_eq!(ev, vec![AgentState::Unknown]);
}

#[test]
fn last_session_gone_is_reported_and_agents_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "claude")]);
    }
    let mut s = server(&fake, dir.path());
    assert_eq!(events(&s.tick(0)).len(), 1);
    fake.0.borrow_mut().sessions = vec![];
    let out = s.tick(2_000);
    assert!(
        out.iter()
            .any(|m| matches!(m, ProbeMsg::Sessions { sessions } if sessions.is_empty())),
        "{out:?}"
    );
    fake.0.borrow_mut().sessions = vec![session("work")];
    let ev: Vec<(AgentState, EventSource)> = events(&s.tick(4_000))
        .iter()
        .map(|e| (e.state, e.source))
        .collect();
    assert_eq!(ev, vec![(AgentState::Unknown, EventSource::Scrape)]);
}

#[test]
fn scraped_agent_leaving_pane_is_reported_exited_once() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "claude")]);
    }
    let mut s = server(&fake, dir.path());
    assert_eq!(events(&s.tick(0)).len(), 1);
    {
        let mut st = fake.0.borrow_mut();
        st.panes.insert(
            "work".into(),
            vec![pane(4, "C:\\WINDOWS\\system32\\cmd.exe")],
        );
        st.screens.insert(4, "C:\\>".into());
    }
    assert!(
        events(&s.tick(3_000)).is_empty(),
        "one miss is not enough to report Exited"
    );
    let ev: Vec<(u32, AgentState, EventSource)> = events(&s.tick(6_000))
        .iter()
        .map(|e| (e.pane.pane_id, e.state, e.source))
        .collect();
    assert_eq!(ev, vec![(4, AgentState::Exited, EventSource::Scrape)]);
    assert!(events(&s.tick(9_000)).is_empty());
}

#[test]
fn prompt_matching_only_needs_input_rules_keeps_scrape_binding() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert(
            "work".into(),
            vec![pane(4, "C:\\WINDOWS\\system32\\cmd.exe")],
        );
        st.screens.insert(4, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let ev: Vec<AgentState> = events(&s.tick(0)).iter().map(|e| e.state).collect();
    assert_eq!(ev, vec![AgentState::Unknown]);
    // A permission prompt that none of claude's screen patterns match.
    fake.0
        .borrow_mut()
        .screens
        .insert(4, " Do you want to proceed?\n > 1. Yes\n".into());
    let mut seen = Vec::new();
    for t in [3_000, 6_000, 9_000, 12_000] {
        seen.extend(events(&s.tick(t)).iter().map(|e| e.state));
    }
    assert_eq!(seen, vec![AgentState::Working, AgentState::NeedsInput]);
}

#[test]
fn hook_exit_blocks_scrape_rebinding_until_agent_leaves_screen() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path());
    spool.append(&stop_record(1, 5)).unwrap();
    spool
        .append(&SpoolRecord {
            payload: json!({"hook_event_name": "SessionEnd"}),
            ..stop_record(2, 5)
        })
        .unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert(
            "work".into(),
            vec![pane(5, "C:\\WINDOWS\\system32\\cmd.exe")],
        );
        // The agent's last screen is still visible after it exited.
        st.screens.insert(5, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let ev: Vec<AgentState> = events(&s.tick(0)).iter().map(|e| e.state).collect();
    assert_eq!(ev, vec![AgentState::Done, AgentState::Exited]);
    assert!(
        events(&s.tick(3_000)).is_empty(),
        "leftover screen must not rebind"
    );

    fake.0.borrow_mut().screens.insert(5, "C:\\>".into());
    assert!(events(&s.tick(6_000)).is_empty());
    fake.0
        .borrow_mut()
        .screens
        .insert(5, fixture("claude-idle"));
    let ev: Vec<(AgentState, EventSource)> = events(&s.tick(9_000))
        .iter()
        .map(|e| (e.state, e.source))
        .collect();
    assert_eq!(ev, vec![(AgentState::Unknown, EventSource::Scrape)]);
}

#[test]
fn persistent_zellij_error_is_reported_once_per_failure() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    fake.0.borrow_mut().sessions = vec![session("work")];
    fake.0.borrow_mut().fail_panes = true;
    let mut s = server(&fake, dir.path());
    assert_eq!(error_codes(&s.tick(0)), vec!["zellij"]);
    assert!(error_codes(&s.tick(2_000)).is_empty());
    fake.0.borrow_mut().fail_panes = false;
    assert!(error_codes(&s.tick(4_000)).is_empty());
    fake.0.borrow_mut().fail_panes = true;
    assert_eq!(error_codes(&s.tick(6_000)), vec!["zellij"]);
}

#[test]
fn old_spool_files_are_cleaned_up_periodically() {
    const DAY: u64 = 86_400_000;
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path());
    spool.append(&stop_record(DAY, 1)).unwrap();
    let mut s = server(&Fake::default(), dir.path());
    let now = 10 * DAY + 20_000;
    s.tick(now);
    assert!(!dir.path().join("1.jsonl").exists());

    spool.append(&stop_record(2 * DAY, 2)).unwrap();
    s.tick(now + 1_000);
    assert!(dir.path().join("2.jsonl").exists(), "not due yet");
    s.tick(now + CLEANUP_EVERY_MS);
    assert!(!dir.path().join("2.jsonl").exists());
}

#[test]
fn session_end_hook_unbinds_pane() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path());
    spool.append(&stop_record(1, 5)).unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert(
            "work".into(),
            vec![pane(5, "C:\\WINDOWS\\system32\\cmd.exe")],
        );
        st.screens.insert(5, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let ev: Vec<AgentState> = events(&s.tick(0)).iter().map(|e| e.state).collect();
    assert_eq!(ev, vec![AgentState::Done]);

    spool
        .append(&SpoolRecord {
            payload: json!({"hook_event_name": "SessionEnd"}),
            ..stop_record(2, 5)
        })
        .unwrap();
    fake.0.borrow_mut().screens.insert(5, "C:\\>".into());
    let ev: Vec<(AgentState, EventSource)> = events(&s.tick(3_000))
        .iter()
        .map(|e| (e.state, e.source))
        .collect();
    assert_eq!(ev, vec![(AgentState::Exited, EventSource::Hook)]);
    assert!(events(&s.tick(6_000)).is_empty());
    assert!(events(&s.tick(12_000)).is_empty());
}

#[test]
fn focus_and_paste_go_to_zellij() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    let mut s = server(&fake, dir.path());
    assert!(
        s.handle(AppMsg::Focus {
            session: "work".into(),
            pane_id: 14,
            tab_id: 2
        })
        .is_empty()
    );
    assert!(
        s.handle(AppMsg::SendText {
            session: "work".into(),
            pane_id: 14,
            text: "yes".into()
        })
        .is_empty()
    );
    assert_eq!(
        fake.0.borrow().calls,
        vec!["focus work 2 14", "paste work 14 yes"]
    );
}

#[test]
fn bad_line_is_reported_and_next_line_works() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = server(&Fake::default(), dir.path());
    assert_eq!(error_codes(&s.handle_line("garbage")), vec!["bad_message"]);
    assert!(s.handle_line("").is_empty());
    let msg = AppMsg::SetInterval {
        pane_poll_ms: 1,
        scrape_ms: 2,
        metrics_ms: 3,
    };
    assert!(s.handle_line(&encode_line(&msg).unwrap()).is_empty());
    assert_eq!(
        s.intervals(),
        Intervals {
            pane_poll_ms: 250,
            scrape_ms: 250,
            metrics_ms: 250
        }
    );
}

#[test]
fn set_interval_keeps_values_above_minimum() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = server(&Fake::default(), dir.path());
    s.handle(AppMsg::SetInterval {
        pane_poll_ms: 0,
        scrape_ms: 5_000,
        metrics_ms: 250,
    });
    assert_eq!(
        s.intervals(),
        Intervals {
            pane_poll_ms: 250,
            scrape_ms: 5_000,
            metrics_ms: 250
        }
    );
}

#[test]
fn bad_rules_are_rejected_and_old_rules_kept() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "claude")]);
    }
    let mut s = server(&fake, dir.path());
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
    assert_eq!(
        error_codes(&s.handle(AppMsg::SetRules { rules: bad })),
        vec!["bad_rules"]
    );
    assert_eq!(events(&s.tick(0))[0].agent, "claude");
}

#[test]
fn set_rules_resets_scrape_history() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "claude")]);
        st.screens.insert(4, "a".into());
    }
    let mut s = server(&fake, dir.path());
    s.tick(0);
    fake.0.borrow_mut().screens.insert(4, "b".into());
    assert!(
        s.handle(AppMsg::SetRules {
            rules: default_rules()
        })
        .is_empty()
    );
    assert!(
        events(&s.tick(3_000)).is_empty(),
        "first dump after reset is a baseline"
    );
    fake.0.borrow_mut().screens.insert(4, "c".into());
    let ev: Vec<AgentState> = events(&s.tick(6_000)).iter().map(|e| e.state).collect();
    assert_eq!(ev, vec![AgentState::Working]);
}

#[test]
fn zellij_errors_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    fake.0.borrow_mut().sessions = vec![session("work")];
    fake.0.borrow_mut().fail_panes = true;
    let mut s = server(&fake, dir.path());
    assert_eq!(error_codes(&s.tick(0)), vec!["zellij"]);
}

#[test]
fn without_zellij_only_spool_events_flow() {
    let dir = tempfile::tempdir().unwrap();
    Spool::new(dir.path()).append(&stop_record(1, 3)).unwrap();
    let mut s: Server<Fake> = Server::new(None, Spool::new(dir.path()), &default_rules()).unwrap();
    let out = s.tick(0);
    assert_eq!(out.len(), 1);
    assert_eq!(events(&out).len(), 1);
    let codes = error_codes(&s.handle(AppMsg::Focus {
        session: "w".into(),
        pane_id: 1,
        tab_id: 0,
    }))
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    assert_eq!(codes, vec!["zellij_missing"]);
}

#[test]
fn spool_read_error_is_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("spool");
    std::fs::write(&p, b"x").unwrap();
    let mut s = Server::new(Some(Fake::default()), Spool::new(&p), &default_rules()).unwrap();
    let out1 = s.tick(0);
    let out2 = s.tick(10);
    let codes: Vec<&str> = error_codes(&out1)
        .into_iter()
        .chain(error_codes(&out2))
        .collect();
    let spool_read_count = codes.iter().filter(|c| *c == &"spool_read").count();
    assert_eq!(
        spool_read_count, 1,
        "spool_read error should appear exactly once"
    );
}

#[test]
fn hook_pane_missing_from_poll_is_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    Spool::new(dir.path()).append(&stop_record(1, 5)).unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![]);
        st.screens.insert(5, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let out1 = s.tick(0);
    assert!(
        !events(&out1).is_empty(),
        "first tick should have hook event"
    );

    fake.0.borrow_mut().panes.insert(
        "work".into(),
        vec![pane(5, "C:\\WINDOWS\\system32\\cmd.exe")],
    );
    let out2 = s.tick(3_000);
    let ev = events(&out2);
    let scrape_unknown = ev.iter().any(|e| {
        e.state == AgentState::Unknown && e.source == EventSource::Scrape && e.pane.pane_id == 5
    });
    assert!(
        scrape_unknown,
        "stale hook entry should be pruned, pane identified afresh by screen"
    );
}
