//! Host tasks and the manager against a scripted connector and in-memory
//! probes. Time is paused: backoff waits and timeouts pass instantly.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mai_core::host::{
    Backoff, ConnState, Connector, HostCommand, HostConfig, HostEvent, HostKind, HostState,
    OpenError, Opened, Problem, STABLE_AFTER, host_state, run_host,
};
use mai_core::link::ProbeIo;
use mai_core::manager::HostManager;
use mai_core::monitor::Update;
use mai_core::pty::{PtyEnds, PtyIn, PtyIo, TermSize, pty_channels};
use mai_core::stderr::StderrTail;
use mai_core::term::{AttachSpec, TermError, TermEvent, TermTransport};
use mai_core::tracker::{AgentKey, TrackerConfig};
use mai_protocol::{
    AgentEvent, AgentState, AppMsg, EventSource, PROTOCOL_VERSION, PaneRef, ProbeMsg, decode_line,
    encode_line,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// The probe's ends of one started `serve`.
struct FakeProbe {
    out: DuplexStream,
    input: Lines<BufReader<DuplexStream>>,
}

impl FakeProbe {
    async fn send(&mut self, msg: &ProbeMsg) {
        let line = encode_line(msg).unwrap();
        self.out.write_all(line.as_bytes()).await.unwrap();
    }

    async fn hello(&mut self, version: u32) {
        self.hello_with(version, None).await;
    }

    async fn hello_with(&mut self, version: u32, zellij: Option<&str>) {
        self.send(&ProbeMsg::Hello {
            protocol_version: version,
            probe_version: "0.1.0".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            zellij_path: zellij.map(str::to_owned),
            zellij_version: None,
        })
        .await;
    }

    async fn heard(&mut self) -> AppMsg {
        let line = timeout(Duration::from_secs(60), self.input.next_line())
            .await
            .expect("app wrote a line")
            .unwrap()
            .unwrap();
        decode_line(&line).unwrap()
    }
}

enum Step {
    Fail(OpenError),
    Probe,
}

/// One terminal attach: the zellij binary used, what was attached, and
/// the PTY ends to drive it.
struct Attach {
    zellij: Option<String>,
    spec: AttachSpec,
    ends: PtyEnds,
}

/// Terminal connection of `Scripted`: every attach succeeds.
struct ScriptedTerms {
    attaches: UnboundedSender<Attach>,
}

impl TermTransport for ScriptedTerms {
    async fn attach(
        &mut self,
        zellij: Option<&str>,
        spec: &AttachSpec,
    ) -> Result<PtyIo, OpenError> {
        let (io, ends) = pty_channels();
        let _ = self.attaches.send(Attach {
            zellij: zellij.map(str::to_owned),
            spec: spec.clone(),
            ends,
        });
        Ok(io)
    }
}

/// Replays `steps` in order; hangs once they run out.
struct Scripted {
    steps: Mutex<VecDeque<Step>>,
    opens: AtomicUsize,
    probes: UnboundedSender<FakeProbe>,
    attaches: UnboundedSender<Attach>,
    attaches_rx: Mutex<Option<UnboundedReceiver<Attach>>>,
    /// Stderr of every probe this connector starts.
    stderr: Arc<StderrTail>,
    /// Notes every probe this connector starts reports.
    notes: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(steps: Vec<Step>) -> (Arc<Self>, UnboundedReceiver<FakeProbe>) {
        let (probes, rx) = mpsc::unbounded_channel();
        let (attaches, attaches_rx) = mpsc::unbounded_channel();
        let c = Self {
            steps: Mutex::new(steps.into()),
            opens: AtomicUsize::new(0),
            probes,
            attaches,
            attaches_rx: Mutex::new(Some(attaches_rx)),
            stderr: Arc::default(),
            notes: Mutex::default(),
        };
        (Arc::new(c), rx)
    }

    fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    /// Terminal attaches made through this connector.
    fn attaches(&self) -> UnboundedReceiver<Attach> {
        self.attaches_rx.lock().unwrap().take().expect("taken once")
    }
}

impl Connector for Scripted {
    type Terminals = ScriptedTerms;

    async fn open_terminals(&self, _host: &HostConfig) -> Result<ScriptedTerms, OpenError> {
        Ok(ScriptedTerms {
            attaches: self.attaches.clone(),
        })
    }

    async fn open(&self, _host: &HostConfig) -> Result<Opened, OpenError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let step = self.steps.lock().unwrap().pop_front();
        match step {
            None => std::future::pending().await,
            Some(Step::Fail(e)) => Err(e),
            Some(Step::Probe) => {
                let (app_read, out) = tokio::io::duplex(64 * 1024);
                let (app_write, probe_in) = tokio::io::duplex(64 * 1024);
                let probe = FakeProbe {
                    out,
                    input: BufReader::new(probe_in).lines(),
                };
                self.probes.send(probe).unwrap();
                Ok(Opened {
                    io: ProbeIo {
                        reader: Box::new(BufReader::new(app_read)),
                        writer: Box::new(app_write),
                    },
                    hooks: Ok(Vec::new()),
                    keep: Box::new(()),
                    notes: self.notes.lock().unwrap().clone(),
                    stderr: self.stderr.clone(),
                })
            }
        }
    }
}

fn cfg(id: &str) -> HostConfig {
    HostConfig {
        id: id.into(),
        kind: HostKind::Local,
        zellij: None,
    }
}

fn retry(m: &str) -> Step {
    Step::Fail(OpenError::Retry(m.into()))
}

struct Harness {
    events: UnboundedReceiver<(String, HostEvent)>,
    cmds: UnboundedSender<HostCommand>,
    probes: UnboundedReceiver<FakeProbe>,
    task: JoinHandle<()>,
}

impl Harness {
    fn start(connector: Arc<Scripted>, probes: UnboundedReceiver<FakeProbe>) -> Self {
        let (ev_tx, events) = mpsc::unbounded_channel();
        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run_host(cfg("h"), connector, ev_tx, cmd_rx));
        Self {
            events,
            cmds,
            probes,
            task,
        }
    }

    async fn event(&mut self) -> HostEvent {
        let (id, ev) = timeout(Duration::from_secs(600), self.events.recv())
            .await
            .expect("host event")
            .expect("host task alive");
        assert_eq!(id, "h");
        ev
    }

    /// Next connection state, skipping other events.
    async fn state(&mut self) -> ConnState {
        loop {
            if let HostEvent::Probe(s) = self.event().await {
                return s;
            }
        }
    }

    async fn probe(&mut self) -> FakeProbe {
        self.probes.recv().await.expect("probe started")
    }
}

fn retrying(s: &ConnState) -> Option<Duration> {
    match s {
        ConnState::Retrying { retry_in, .. } => Some(*retry_in),
        _ => None,
    }
}

fn hook_event(offset: u64, state: AgentState) -> ProbeMsg {
    ProbeMsg::AgentEvent(AgentEvent {
        pane: PaneRef {
            session: "w".into(),
            pane_id: 1,
        },
        agent: "claude".into(),
        source: EventSource::Hook,
        state,
        message: None,
        ts_ms: offset,
        spool_offset: Some(offset),
    })
}

#[test]
fn backoff_doubles_to_a_minute_and_resets() {
    let mut b = Backoff::default();
    let secs: Vec<u64> = (0..8).map(|_| b.next_delay().as_secs()).collect();
    assert_eq!(secs, vec![1, 2, 4, 8, 16, 32, 60, 60]);
    b.reset();
    assert_eq!(b.next_delay(), Duration::from_secs(1));
}

#[test]
fn host_state_combines_connections() {
    let down = ConnState::Retrying {
        retry_in: Duration::from_secs(1),
        reason: "x".into(),
    };
    let auth = ConnState::NeedsUser(Problem::HostKeyRejected);
    let deploy = ConnState::NeedsUser(Problem::Deploy("x".into()));
    assert_eq!(host_state(&ConnState::Up, None), HostState::Online);
    assert_eq!(
        host_state(&ConnState::Up, Some(&ConnState::Up)),
        HostState::Online
    );
    assert_eq!(host_state(&ConnState::Up, Some(&down)), HostState::Degraded);
    assert_eq!(
        host_state(&deploy, Some(&ConnState::Up)),
        HostState::Degraded
    );
    assert_eq!(host_state(&auth, None), HostState::AuthRequired);
    assert_eq!(
        host_state(&ConnState::Connecting, None),
        HostState::Connecting
    );
    assert_eq!(host_state(&down, None), HostState::Offline);
    assert_eq!(host_state(&deploy, None), HostState::Offline);
}

#[tokio::test(start_paused = true)]
async fn connected_probe_relays_messages_and_acks_hook_events() {
    let (c, probes) = Scripted::new(vec![Step::Probe]);
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    let mut probe = h.probe().await;
    probe.hello(PROTOCOL_VERSION).await;
    assert!(matches!(h.event().await, HostEvent::Hello(i) if i.os == "linux"));
    assert_eq!(h.event().await, HostEvent::Hooks(Ok(Vec::new())));
    assert_eq!(h.event().await, HostEvent::Probe(ConnState::Up));

    probe.send(&hook_event(42, AgentState::Done)).await;
    assert!(matches!(
        h.event().await,
        HostEvent::Msg(ProbeMsg::AgentEvent(_))
    ));
    assert_eq!(probe.heard().await, AppMsg::Ack { spool_offset: 42 });

    h.cmds
        .send(HostCommand::Send(AppMsg::Focus {
            session: "w".into(),
            pane_id: 1,
            tab_id: 0,
        }))
        .unwrap();
    assert!(matches!(
        probe.heard().await,
        AppMsg::Focus { pane_id: 1, .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn transient_failures_back_off_then_connect() {
    let (c, probes) = Scripted::new(vec![retry("net down"), retry("net down"), Step::Probe]);
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    let s = h.state().await;
    assert_eq!(retrying(&s), Some(Duration::from_secs(1)));
    assert!(matches!(&s, ConnState::Retrying { reason, .. } if reason == "net down"));
    assert_eq!(h.state().await, ConnState::Connecting);
    assert_eq!(retrying(&h.state().await), Some(Duration::from_secs(2)));
    assert_eq!(h.state().await, ConnState::Connecting);
    h.probe().await.hello(PROTOCOL_VERSION).await;
    assert_eq!(h.state().await, ConnState::Up);
}

#[tokio::test(start_paused = true)]
async fn problem_needing_the_user_waits_for_retry() {
    let (c, probes) = Scripted::new(vec![
        Step::Fail(OpenError::NeedsUser(Problem::Auth("denied".into()))),
        retry("later"),
    ]);
    let mut h = Harness::start(c.clone(), probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    assert_eq!(
        h.state().await,
        ConnState::NeedsUser(Problem::Auth("denied".into()))
    );
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(c.opens(), 1, "no automatic retry");

    h.cmds
        .send(HostCommand::Send(AppMsg::Ack { spool_offset: 1 }))
        .unwrap();
    assert_eq!(
        h.event().await,
        HostEvent::Dropped(AppMsg::Ack { spool_offset: 1 })
    );
    h.cmds.send(HostCommand::Retry).unwrap();
    assert_eq!(h.state().await, ConnState::Connecting);
    assert_eq!(retrying(&h.state().await), Some(Duration::from_secs(1)));
    assert_eq!(c.opens(), 2);
}

#[tokio::test(start_paused = true)]
async fn retry_command_skips_the_backoff_wait() {
    let (c, probes) = Scripted::new(vec![retry("a"), retry("b"), retry("c")]);
    let mut h = Harness::start(c.clone(), probes);
    for _ in 0..4 {
        h.state().await;
    }
    // Now waiting 2 s before the third attempt; Retry starts it at once.
    let before = tokio::time::Instant::now();
    h.cmds.send(HostCommand::Retry).unwrap();
    assert_eq!(h.state().await, ConnState::Connecting);
    assert!(before.elapsed() < Duration::from_secs(2));
}

#[tokio::test(start_paused = true)]
async fn probe_exit_reconnects_and_short_lived_probe_keeps_backing_off() {
    let (c, probes) = Scripted::new(vec![retry("x"), Step::Probe, Step::Probe]);
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    assert_eq!(retrying(&h.state().await), Some(Duration::from_secs(1)));
    assert_eq!(h.state().await, ConnState::Connecting);
    let mut probe = h.probe().await;
    probe.hello(PROTOCOL_VERSION).await;
    assert_eq!(h.state().await, ConnState::Up);
    drop(probe);
    let s = h.state().await;
    assert_eq!(retrying(&s), Some(Duration::from_secs(2)), "{s:?}");
    assert!(matches!(&s, ConnState::Retrying { reason, .. } if reason.contains("closed")));
    assert_eq!(h.state().await, ConnState::Connecting);
}

#[tokio::test(start_paused = true)]
async fn stable_probe_resets_backoff_when_it_drops() {
    let (c, probes) = Scripted::new(vec![retry("x"), retry("x"), Step::Probe]);
    let mut h = Harness::start(c, probes);
    for _ in 0..5 {
        h.state().await;
    }
    let mut probe = h.probe().await;
    probe.hello(PROTOCOL_VERSION).await;
    assert_eq!(h.state().await, ConnState::Up);
    let mut waited = Duration::ZERO;
    while waited <= STABLE_AFTER {
        probe.send(&ProbeMsg::Heartbeat { ts_ms: 1 }).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        waited += Duration::from_secs(5);
    }
    drop(probe);
    assert_eq!(retrying(&h.state().await), Some(Duration::from_secs(1)));
}

#[tokio::test(start_paused = true)]
async fn other_protocol_version_needs_the_user() {
    let (c, probes) = Scripted::new(vec![Step::Probe]);
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    h.probe().await.hello(PROTOCOL_VERSION + 1).await;
    assert_eq!(
        h.state().await,
        ConnState::NeedsUser(Problem::Protocol {
            probe: PROTOCOL_VERSION + 1,
            app: PROTOCOL_VERSION
        })
    );
}

#[tokio::test(start_paused = true)]
async fn stop_ends_the_task_even_while_connecting() {
    let (c, probes) = Scripted::new(vec![]);
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    h.cmds.send(HostCommand::Stop).unwrap();
    timeout(Duration::from_secs(5), h.task)
        .await
        .expect("task ended")
        .unwrap();
}

async fn next_update(rx: &mut UnboundedReceiver<Update>) -> Update {
    timeout(Duration::from_secs(600), rx.recv())
        .await
        .expect("update")
        .expect("monitor alive")
}

/// Skip updates until `f` matches one.
async fn until<T>(rx: &mut UnboundedReceiver<Update>, f: impl Fn(&Update) -> Option<T>) -> T {
    loop {
        if let Some(t) = f(&next_update(rx).await) {
            return t;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn manager_turns_probe_events_into_updates() {
    let (c, mut probes) = Scripted::new(vec![Step::Probe]);
    let (mut mgr, mut rx) = HostManager::start(c, TrackerConfig::default());
    assert!(mgr.add_host(cfg("h")));
    assert!(!mgr.add_host(cfg("h")), "duplicate id");
    assert_eq!(mgr.host_ids(), vec!["h".to_owned()]);

    let mut probe = probes.recv().await.unwrap();
    probe.hello(PROTOCOL_VERSION).await;
    let state = until(&mut rx, |u| match u {
        Update::Host { state, .. } if *state == HostState::Online => Some(*state),
        _ => None,
    })
    .await;
    assert_eq!(state, HostState::Online);

    probe.send(&hook_event(7, AgentState::NeedsInput)).await;
    let alert = until(&mut rx, |u| match u {
        Update::Alert(a) => Some(a.clone()),
        _ => None,
    })
    .await;
    assert_eq!(alert.state, AgentState::NeedsInput);
    assert_eq!(probe.heard().await, AppMsg::Ack { spool_offset: 7 });

    mgr.acknowledge(AgentKey {
        host_id: "h".into(),
        session: "w".into(),
        pane_id: 1,
    });
    let acked = until(&mut rx, |u| match u {
        Update::Agent(r) => Some(r.acknowledged),
        _ => None,
    })
    .await;
    assert!(acked);

    assert!(mgr.send(
        "h",
        AppMsg::SendText {
            session: "w".into(),
            pane_id: 1,
            text: "yes".into()
        }
    ));
    assert!(matches!(probe.heard().await, AppMsg::SendText { text, .. } if text == "yes"));
    assert!(!mgr.send("nope", AppMsg::Ack { spool_offset: 1 }));
}

#[tokio::test(start_paused = true)]
async fn removed_host_can_be_added_again() {
    let (c, mut probes) = Scripted::new(vec![Step::Probe, Step::Probe]);
    let (mut mgr, mut rx) = HostManager::start(c.clone(), TrackerConfig::default());
    mgr.add_host(cfg("h"));
    let _first = probes.recv().await.unwrap();
    assert!(mgr.remove_host("h"));
    assert!(!mgr.remove_host("h"));
    assert!(mgr.host_ids().is_empty());

    assert!(mgr.add_host(cfg("h")));
    let mut second = probes.recv().await.unwrap();
    second.hello(PROTOCOL_VERSION).await;
    let state = until(&mut rx, |u| match u {
        Update::Host { state, .. } if *state == HostState::Online => Some(*state),
        _ => None,
    })
    .await;
    assert_eq!(state, HostState::Online);
    assert_eq!(c.opens(), 2);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_manager_ends_the_update_stream() {
    let (c, _probes) = Scripted::new(vec![]);
    let (mut mgr, mut rx) = HostManager::start(c, TrackerConfig::default());
    mgr.add_host(cfg("h"));
    drop(mgr);
    loop {
        match timeout(Duration::from_secs(60), rx.recv()).await {
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => panic!("update stream did not end"),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn terminal_uses_the_probe_zellij_and_joins_the_host_state() {
    let (c, mut probes) = Scripted::new(vec![Step::Probe]);
    let mut attaches = c.attaches();
    let (mut mgr, mut rx) = HostManager::start(c, TrackerConfig::default());
    mgr.add_host(cfg("h"));
    let size = TermSize { cols: 80, rows: 24 };
    assert_eq!(
        mgr.open_terminal("nope", "w", false, size).await.err(),
        Some(TermError::NoHost)
    );

    let mut probe = probes.recv().await.unwrap();
    probe
        .hello_with(PROTOCOL_VERSION, Some("/usr/bin/zellij"))
        .await;
    until(&mut rx, |u| match u {
        Update::Hello { .. } => Some(()),
        _ => None,
    })
    .await;

    let mut term = mgr.open_terminal("h", "work", false, size).await.unwrap();
    assert_eq!(term.recv().await, Some(TermEvent::Attached));
    let mut pty = attaches.recv().await.unwrap();
    assert_eq!(pty.zellij.as_deref(), Some("/usr/bin/zellij"));
    assert_eq!(pty.spec.session, "work");
    let term_up = until(&mut rx, |u| match u {
        Update::Host {
            term: Some(t),
            state,
            ..
        } => Some((t.clone(), *state)),
        _ => None,
    })
    .await;
    assert_eq!(term_up, (ConnState::Up, HostState::Online));

    term.write(b"q".to_vec());
    assert_eq!(
        pty.ends.input.recv().await,
        Some(PtyIn::Data(b"q".to_vec()))
    );
    drop(term);
    assert_eq!(pty.ends.input.recv().await, Some(PtyIn::Close));
    // The last terminal is gone: the terminal connection is closed.
    until(&mut rx, |u| match u {
        Update::Host { term: None, .. } => Some(()),
        _ => None,
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn probe_stderr_explains_why_it_stopped() {
    let (c, probes) = Scripted::new(vec![Step::Probe]);
    let mut h = Harness::start(c.clone(), probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    let probe = h.probe().await;
    c.stderr
        .push(b"mai-probe serve: rules.toml: invalid regex\n");
    c.stderr.finish();
    drop(probe);
    match h.state().await {
        ConnState::Retrying { reason, .. } => {
            assert!(reason.contains("closed"), "{reason}");
            assert!(
                reason.contains("probe stderr: mai-probe serve: rules.toml: invalid regex"),
                "{reason}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn dropping_the_manager_ends_open_terminals() {
    let (c, _probes) = Scripted::new(vec![]);
    let mut attaches = c.attaches();
    let (mut mgr, _rx) = HostManager::start(c, TrackerConfig::default());
    mgr.add_host(cfg("h"));
    let size = TermSize { cols: 80, rows: 24 };
    let mut term = mgr.open_terminal("h", "work", false, size).await.unwrap();
    assert_eq!(term.recv().await, Some(TermEvent::Attached));
    let mut pty = attaches.recv().await.unwrap();
    drop(mgr);
    assert_eq!(pty.ends.input.recv().await, Some(PtyIn::Close));
    let end = timeout(Duration::from_secs(60), term.recv())
        .await
        .expect("terminal stream ended");
    assert_eq!(end, None);
}

#[tokio::test(start_paused = true)]
async fn connect_notes_are_reported_after_hello() {
    let (c, probes) = Scripted::new(vec![Step::Probe]);
    c.notes
        .lock()
        .unwrap()
        .push("ssh config line 3: Match blocks are not supported".into());
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    h.probe().await.hello(PROTOCOL_VERSION).await;
    assert!(matches!(h.event().await, HostEvent::Hello(_)));
    assert!(matches!(h.event().await, HostEvent::Hooks(_)));
    assert_eq!(
        h.event().await,
        HostEvent::Notes(vec![
            "ssh config line 3: Match blocks are not supported".into()
        ])
    );
    assert_eq!(h.event().await, HostEvent::Probe(ConnState::Up));
}
