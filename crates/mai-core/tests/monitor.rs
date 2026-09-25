//! `Monitor`: host events in, UI updates out.

use mai_core::host::{ConnState, HostEvent, HostState, Problem};
use mai_core::monitor::{Monitor, Update};
use mai_core::tracker::{AgentKey, TrackerConfig};
use mai_protocol::{
    AgentEvent, AgentState, EventSource, Metrics, PaneInfo, PaneRef, ProbeMsg, SessionInfo,
};

fn monitor() -> Monitor {
    Monitor::new(TrackerConfig::default())
}

fn event(session: &str, pane_id: u32, state: AgentState, ts_ms: u64) -> HostEvent {
    HostEvent::Msg(ProbeMsg::AgentEvent(AgentEvent {
        pane: PaneRef {
            session: session.into(),
            pane_id,
        },
        agent: "claude".into(),
        source: EventSource::Hook,
        state,
        message: None,
        ts_ms,
        spool_offset: Some(ts_ms),
    }))
}

fn pane(id: u32) -> PaneInfo {
    PaneInfo {
        id,
        tab_id: 0,
        tab_name: "t".into(),
        title: "x".into(),
        command: None,
        exited: false,
    }
}

fn key(session: &str, pane_id: u32) -> AgentKey {
    AgentKey {
        host_id: "h".into(),
        session: session.into(),
        pane_id,
    }
}

/// `(pane, state, acknowledged)` of every `Update::Agent`.
fn agents(updates: &[Update]) -> Vec<(u32, AgentState, bool)> {
    updates
        .iter()
        .filter_map(|u| match u {
            Update::Agent(r) => Some((r.key.pane_id, r.state, r.acknowledged)),
            _ => None,
        })
        .collect()
}

fn alerts(updates: &[Update]) -> Vec<AgentState> {
    updates
        .iter()
        .filter_map(|u| match u {
            Update::Alert(a) => Some(a.state),
            _ => None,
        })
        .collect()
}

#[test]
fn connection_state_becomes_host_state() {
    let mut m = monitor();
    let up = m.apply("h", HostEvent::Probe(ConnState::Up), 0);
    assert_eq!(
        up,
        vec![Update::Host {
            id: "h".into(),
            state: HostState::Online,
            probe: ConnState::Up
        }]
    );
    let auth = ConnState::NeedsUser(Problem::Auth("denied".into()));
    let out = m.apply("h", HostEvent::Probe(auth.clone()), 0);
    assert!(matches!(
        &out[0],
        Update::Host {
            state: HostState::AuthRequired,
            ..
        }
    ));
    assert_eq!(m.host("h").unwrap().probe, Some(auth));
}

#[test]
fn agent_changes_and_alerts_are_reported_once() {
    let mut m = monitor();
    let out = m.apply("h", event("w", 1, AgentState::Working, 10), 10);
    assert_eq!(agents(&out), vec![(1, AgentState::Working, true)]);
    assert!(alerts(&out).is_empty());

    let out = m.apply("h", event("w", 1, AgentState::NeedsInput, 20), 20);
    assert_eq!(agents(&out), vec![(1, AgentState::NeedsInput, false)]);
    assert_eq!(alerts(&out), vec![AgentState::NeedsInput]);

    let out = m.apply("h", event("w", 1, AgentState::NeedsInput, 30), 30);
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn acknowledge_reports_the_change_once() {
    let mut m = monitor();
    m.apply("h", event("w", 1, AgentState::Done, 10), 10);
    let u = m.acknowledge(&key("w", 1)).unwrap();
    assert_eq!(agents(&[u]), vec![(1, AgentState::Done, true)]);
    assert!(m.acknowledge(&key("w", 1)).is_none());
    assert!(m.acknowledge(&key("w", 99)).is_none());
}

#[test]
fn vanished_or_exited_pane_ends_its_agent() {
    let mut m = monitor();
    m.apply("h", event("w", 1, AgentState::Working, 10), 10);
    m.apply("h", event("w", 2, AgentState::Working, 10), 10);
    m.apply("h", event("x", 3, AgentState::Working, 10), 10);
    let mut exited = pane(2);
    exited.exited = true;
    let out = m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Panes {
            session: "w".into(),
            panes: vec![exited],
        }),
        50,
    );
    let mut gone = agents(&out);
    gone.sort_by_key(|g| g.0);
    assert_eq!(
        gone,
        vec![(1, AgentState::Exited, true), (2, AgentState::Exited, true)]
    );
    assert!(matches!(&out[0], Update::Panes { session, .. } if session == "w"));
    assert_eq!(
        m.tracker().get(&key("x", 3)).unwrap().state,
        AgentState::Working
    );

    let again = m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Panes {
            session: "w".into(),
            panes: vec![],
        }),
        60,
    );
    assert!(
        agents(&again).is_empty(),
        "already exited agents stay quiet"
    );
}

#[test]
fn ended_session_ends_its_agents_and_panes() {
    let mut m = monitor();
    m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Panes {
            session: "x".into(),
            panes: vec![pane(3)],
        }),
        0,
    );
    m.apply("h", event("x", 3, AgentState::Done, 10), 10);
    m.apply("h", event("w", 1, AgentState::Working, 10), 10);
    let out = m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Sessions {
            sessions: vec![
                SessionInfo {
                    name: "w".into(),
                    exited: false,
                },
                SessionInfo {
                    name: "x".into(),
                    exited: true,
                },
            ],
        }),
        20,
    );
    assert_eq!(agents(&out), vec![(3, AgentState::Exited, true)]);
    assert!(m.host("h").unwrap().panes.is_empty());
    assert!(
        m.tracker().pending().is_empty(),
        "exited agent no longer alerts"
    );
}

#[test]
fn agents_of_other_hosts_are_untouched() {
    let mut m = monitor();
    m.apply("other", event("w", 1, AgentState::Working, 10), 10);
    let out = m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Sessions { sessions: vec![] }),
        20,
    );
    assert!(agents(&out).is_empty());
}

#[test]
fn metrics_errors_and_heartbeats() {
    let mut m = monitor();
    let metrics = Metrics {
        cpu_pct: 12.5,
        mem_used: 1,
        mem_total: 2,
        disk_read_bps: 0,
        disk_write_bps: 0,
        net_rx_bps: 0,
        net_tx_bps: 0,
        load1: None,
        ts_ms: 5,
    };
    let out = m.apply("h", HostEvent::Msg(ProbeMsg::Metrics(metrics.clone())), 5);
    assert_eq!(
        out,
        vec![Update::Metrics {
            id: "h".into(),
            metrics: metrics.clone()
        }]
    );
    assert_eq!(m.host("h").unwrap().metrics, Some(metrics));
    let out = m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Error {
            code: "zellij".into(),
            message: "boom".into(),
        }),
        6,
    );
    assert!(matches!(&out[0], Update::ProbeError { code, .. } if code == "zellij"));
    assert!(
        m.apply("h", HostEvent::Msg(ProbeMsg::Heartbeat { ts_ms: 7 }), 7)
            .is_empty()
    );
}
