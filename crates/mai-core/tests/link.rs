//! `ProbeLink` against an in-memory probe (tokio duplex pipes).

use mai_core::link::{HELLO_TIMEOUT, LinkError, ProbeIo, ProbeLink, SILENCE_TIMEOUT};
use mai_protocol::{AppMsg, PROTOCOL_VERSION, ProbeMsg, decode_line, encode_line};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

/// The probe's ends: write its stdout, read its stdin.
struct FakeProbe {
    out: DuplexStream,
    input: tokio::io::Lines<BufReader<DuplexStream>>,
}

impl FakeProbe {
    async fn say(&mut self, text: &str) {
        self.out.write_all(text.as_bytes()).await.unwrap();
    }

    async fn send(&mut self, msg: &ProbeMsg) {
        self.say(&encode_line(msg).unwrap()).await;
    }

    async fn heard(&mut self) -> AppMsg {
        decode_line(&self.input.next_line().await.unwrap().unwrap()).unwrap()
    }
}

fn pair() -> (ProbeLink, FakeProbe) {
    let (app_read, probe_out) = tokio::io::duplex(64 * 1024);
    let (app_write, probe_in) = tokio::io::duplex(64 * 1024);
    let link = ProbeLink::new(ProbeIo {
        reader: Box::new(BufReader::new(app_read)),
        writer: Box::new(app_write),
    });
    let probe = FakeProbe {
        out: probe_out,
        input: BufReader::new(probe_in).lines(),
    };
    (link, probe)
}

fn hello(version: u32) -> ProbeMsg {
    ProbeMsg::Hello {
        protocol_version: version,
        probe_version: "0.1.0".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        zellij_path: Some("/usr/bin/zellij".into()),
        zellij_version: Some("0.44.3".into()),
    }
}

#[tokio::test]
async fn hello_is_found_after_shell_noise() {
    let (mut link, mut probe) = pair();
    probe.say("Last login: Mon Sep 22\nwelcome!\n").await;
    probe.send(&hello(PROTOCOL_VERSION)).await;
    let info = link.hello().await.unwrap();
    assert_eq!(info.os, "linux");
    assert_eq!(info.zellij_version.as_deref(), Some("0.44.3"));
}

#[tokio::test]
async fn other_protocol_version_is_refused() {
    let (mut link, mut probe) = pair();
    probe.send(&hello(PROTOCOL_VERSION + 1)).await;
    assert_eq!(
        link.hello().await,
        Err(LinkError::Protocol {
            probe: PROTOCOL_VERSION + 1,
            app: PROTOCOL_VERSION
        })
    );
}

#[tokio::test]
async fn first_message_must_be_hello() {
    let (mut link, mut probe) = pair();
    probe.send(&ProbeMsg::Heartbeat { ts_ms: 1 }).await;
    assert!(matches!(link.hello().await, Err(LinkError::NoHello(_))));
}

#[tokio::test]
async fn endless_noise_is_not_a_probe() {
    let (mut link, mut probe) = pair();
    for i in 0..60 {
        probe.say(&format!("noise {i}\n")).await;
    }
    match link.hello().await {
        Err(LinkError::NoHello(m)) => assert!(m.contains("noise 0"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn silent_probe_times_out_waiting_for_hello() {
    let (mut link, _probe) = pair();
    let start = tokio::time::Instant::now();
    assert_eq!(link.hello().await, Err(LinkError::Silent));
    assert!(start.elapsed() >= HELLO_TIMEOUT);
}

#[tokio::test(start_paused = true)]
async fn silence_after_hello_is_detected() {
    let (mut link, mut probe) = pair();
    probe.send(&hello(PROTOCOL_VERSION)).await;
    link.hello().await.unwrap();
    let start = tokio::time::Instant::now();
    assert_eq!(link.recv().await, Err(LinkError::Silent));
    assert!(start.elapsed() >= SILENCE_TIMEOUT);
}

#[tokio::test]
async fn bad_line_is_an_error_message_and_eof_closes() {
    let (mut link, mut probe) = pair();
    probe.send(&hello(PROTOCOL_VERSION)).await;
    link.hello().await.unwrap();
    probe.say("{broken\n").await;
    probe.send(&ProbeMsg::Heartbeat { ts_ms: 5 }).await;
    match link.recv().await.unwrap() {
        ProbeMsg::Error { code, .. } => assert_eq!(code, "bad_probe_line"),
        other => panic!("{other:?}"),
    }
    assert_eq!(link.recv().await.unwrap(), ProbeMsg::Heartbeat { ts_ms: 5 });
    drop(probe);
    assert_eq!(link.recv().await, Err(LinkError::Closed));
}

#[tokio::test]
async fn sent_messages_arrive_as_lines() {
    let (mut link, mut probe) = pair();
    link.send(&AppMsg::Ack { spool_offset: 9 }).await.unwrap();
    assert_eq!(probe.heard().await, AppMsg::Ack { spool_offset: 9 });
}
