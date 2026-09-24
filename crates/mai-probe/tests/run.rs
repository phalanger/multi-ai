use std::io::Cursor;

use mai_probe::metrics::MetricsSampler;
use mai_probe::rules::default_rules;
use mai_probe::run::run;
use mai_probe::serve::Server;
use mai_probe::spool::Spool;
use mai_probe::zellij::CliZellij;
use mai_protocol::{PROTOCOL_VERSION, ProbeMsg, decode_line};

fn hello() -> ProbeMsg {
    ProbeMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        probe_version: "test".into(),
        os: "test".into(),
        arch: "test".into(),
        zellij_path: None,
        zellij_version: None,
    }
}

#[test]
fn run_sends_hello_answers_input_and_stops_at_eof() {
    let dir = tempfile::tempdir().unwrap();
    let server: Server<CliZellij> =
        Server::new(None, Spool::new(dir.path()), &default_rules()).unwrap();
    let mut out = Vec::new();
    run(
        server,
        hello(),
        Cursor::new(b"garbage\n".to_vec()),
        &mut out,
    )
    .unwrap();
    let msgs: Vec<ProbeMsg> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| decode_line(l).unwrap())
        .collect();
    assert_eq!(msgs[0], hello());
    assert!(
        msgs.iter()
            .any(|m| matches!(m, ProbeMsg::Error { code, .. } if code == "bad_message"))
    );
    assert!(msgs.iter().any(|m| matches!(m, ProbeMsg::Heartbeat { .. })));
    assert!(msgs.iter().any(|m| matches!(m, ProbeMsg::Metrics(_))));
}

#[test]
fn non_utf8_line_is_reported_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let server: Server<CliZellij> =
        Server::new(None, Spool::new(dir.path()), &default_rules()).unwrap();
    let mut out = Vec::new();
    run(server, hello(), Cursor::new(vec![0xff, b'\n']), &mut out).unwrap();
    let msgs: Vec<ProbeMsg> = String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| decode_line(l).unwrap())
        .collect();
    assert!(
        msgs.iter()
            .any(|m| matches!(m, ProbeMsg::Error { code, .. } if code == "bad_message")),
        "{msgs:?}"
    );
}

#[test]
fn metrics_sample_is_plausible() {
    let mut m = MetricsSampler::new();
    m.sample(1_000);
    let s = m.sample(2_000);
    assert!(s.mem_total > 0 && s.mem_used <= s.mem_total);
    assert!((0.0..=100.0).contains(&s.cpu_pct), "{}", s.cpu_pct);
    assert_eq!(s.ts_ms, 2_000);
    assert_eq!(s.load1.is_none(), cfg!(windows));
}
