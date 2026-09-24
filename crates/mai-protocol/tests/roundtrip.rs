use mai_protocol::{
    AgentEvent, AgentRule, AgentState, AppMsg, EventSource, Metrics,
    PaneInfo, PaneRef, ProbeMsg, ScrapeRules, SessionInfo, decode_line,
    encode_line,
};

fn sample_event() -> AgentEvent {
    AgentEvent {
        pane: PaneRef { session: "work".into(), pane_id: 3 },
        agent: "claude".into(),
        source: EventSource::Hook,
        state: AgentState::NeedsInput,
        message: Some("needs permission".into()),
        ts_ms: 1_700_000_000_000,
        spool_offset: Some(42),
    }
}

fn roundtrip_probe(msg: ProbeMsg) {
    let line = encode_line(&msg).unwrap();
    assert!(line.ends_with('\n'));
    assert_eq!(line.matches('\n').count(), 1, "one line: {line}");
    let back: ProbeMsg = decode_line(&line).unwrap();
    assert_eq!(back, msg);
}

fn roundtrip_app(msg: AppMsg) {
    let line = encode_line(&msg).unwrap();
    assert_eq!(line.matches('\n').count(), 1, "one line: {line}");
    let back: AppMsg = decode_line(&line).unwrap();
    assert_eq!(back, msg);
}

#[test]
fn probe_messages_roundtrip() {
    roundtrip_probe(ProbeMsg::Hello {
        protocol_version: mai_protocol::PROTOCOL_VERSION,
        probe_version: "0.1.0+abc".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        zellij_path: None,
        zellij_version: Some("0.44.3".into()),
    });
    roundtrip_probe(ProbeMsg::Sessions {
        sessions: vec![SessionInfo { name: "work".into(), exited: false }],
    });
    roundtrip_probe(ProbeMsg::Panes {
        session: "work".into(),
        panes: vec![PaneInfo {
            id: 3,
            tab_id: 1,
            tab_name: "t".into(),
            title: "claude".into(),
            command: None,
            exited: false,
        }],
    });
    roundtrip_probe(ProbeMsg::AgentEvent(sample_event()));
    roundtrip_probe(ProbeMsg::Metrics(Metrics {
        cpu_pct: 12.5,
        mem_used: 1,
        mem_total: 2,
        disk_read_bps: 3,
        disk_write_bps: 4,
        net_rx_bps: 5,
        net_tx_bps: 6,
        load1: None,
        ts_ms: 7,
    }));
    roundtrip_probe(ProbeMsg::Heartbeat { ts_ms: 9 });
    roundtrip_probe(ProbeMsg::Error {
        code: "zellij_missing".into(),
        message: "not found".into(),
    });
}

#[test]
fn app_messages_roundtrip() {
    roundtrip_app(AppMsg::Ack { spool_offset: 42 });
    roundtrip_app(AppMsg::SendText {
        session: "work".into(),
        pane_id: 3,
        text: "line1\nline2".into(),
    });
    roundtrip_app(AppMsg::Focus {
        session: "work".into(),
        pane_id: 3,
        tab_id: 1,
    });
    roundtrip_app(AppMsg::SetInterval {
        pane_poll_ms: 2000,
        scrape_ms: 3000,
        metrics_ms: 2000,
    });
    roundtrip_app(AppMsg::SetRules {
        rules: ScrapeRules {
            agents: vec![AgentRule {
                name: "fake".into(),
                title_patterns: vec!["^fake".into()],
                screen_patterns: vec![],
                needs_input_patterns: vec!["y/n".into()],
                done_patterns: vec![],
                ignore_patterns: vec![],
                stable_ms: 5000,
            }],
        },
    });
}

#[test]
fn agent_event_is_flat_with_type_tag() {
    let line = encode_line(&ProbeMsg::AgentEvent(sample_event())).unwrap();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["type"], "agent_event");
    assert_eq!(v["state"], "needs_input");
    assert_eq!(v["source"], "hook");
    assert_eq!(v["pane"]["pane_id"], 3);
}

#[test]
fn optional_fields_may_be_absent() {
    let line = r#"{"type":"agent_event","pane":{"session":"s","pane_id":1},
        "agent":"codex","source":"scrape","state":"done","ts_ms":5}"#
        .replace('\n', "");
    let msg: ProbeMsg = decode_line(&line).unwrap();
    let ProbeMsg::AgentEvent(ev) = msg else {
        panic!("wrong variant")
    };
    assert_eq!(ev.message, None);
    assert_eq!(ev.spool_offset, None);
}

#[test]
fn decode_accepts_crlf() {
    let msg: ProbeMsg =
        decode_line("{\"type\":\"heartbeat\",\"ts_ms\":1}\r\n").unwrap();
    assert_eq!(msg, ProbeMsg::Heartbeat { ts_ms: 1 });
}

#[test]
fn unknown_type_is_error() {
    let r: Result<ProbeMsg, _> = decode_line("{\"type\":\"nope\"}");
    assert!(r.is_err());
}
