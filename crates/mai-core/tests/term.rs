//! Terminal task against a scripted connector whose PTYs the test drives,
//! plus the attach command lines. Time is paused: backoff waits pass
//! instantly.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mai_core::deploy::{Os, Remote, Shell};
use mai_core::host::{
    ConnState, Connector, HostConfig, HostEvent, HostKind, OpenError, Opened, Problem,
};
use mai_core::pty::{PtyEnds, PtyIn, PtyIo, PtyOut, TermSize, pty_channels};
use mai_core::term::{
    AttachSpec, OpenResult, TermCmd, TermError, TermEvent, TermTransport, ZellijPaths, attach_argv,
    remote_attach_command, run_terms,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::timeout;

const SIZE: TermSize = TermSize { cols: 80, rows: 24 };

fn remote(os: Os, shell: Shell) -> Remote {
    Remote {
        os,
        arch: "x86_64".into(),
        shell,
        home: "h".into(),
    }
}

#[test]
fn attach_arguments() {
    assert_eq!(attach_argv("work", false), vec!["attach", "work"]);
    assert_eq!(attach_argv("new", true), vec!["attach", "--create", "new"]);
}

#[test]
fn remote_attach_command_per_shell_and_locale() {
    let posix = remote(Os::MacOs, Shell::Posix);
    assert_eq!(
        remote_attach_command(
            &posix,
            Some("/opt/homebrew/bin/zellij"),
            "work",
            false,
            true
        ),
        "'/opt/homebrew/bin/zellij' attach work"
    );
    assert_eq!(
        remote_attach_command(&posix, None, "my session", true, false),
        "LANG=en_US.UTF-8 LC_CTYPE=en_US.UTF-8 'zellij' attach --create 'my session'"
    );
    let cmd = remote(Os::Windows, Shell::Cmd);
    assert_eq!(
        remote_attach_command(&cmd, Some(r"D:\Apps\zellij\zellij.exe"), "w", false, false),
        r#""D:\Apps\zellij\zellij.exe" attach w"#
    );
    let ps = remote(Os::Windows, Shell::PowerShell);
    assert_eq!(
        remote_attach_command(&ps, None, "w", false, false),
        "& 'zellij' attach w"
    );
}

/// One `attach` the terminal task made, and the PTY ends to drive it.
struct Attached {
    spec: AttachSpec,
    zellij: Option<String>,
    ends: PtyEnds,
}

struct FakeTerms {
    attaches: UnboundedSender<Attached>,
}

impl TermTransport for FakeTerms {
    async fn attach(
        &mut self,
        zellij: Option<&str>,
        spec: &AttachSpec,
    ) -> Result<PtyIo, OpenError> {
        let (io, ends) = pty_channels();
        let a = Attached {
            spec: spec.clone(),
            zellij: zellij.map(str::to_owned),
            ends,
        };
        self.attaches.send(a).unwrap();
        Ok(io)
    }
}

/// `open_terminals` fails with the queued errors first, then succeeds.
struct FakeConnector {
    failures: Mutex<VecDeque<OpenError>>,
    opens: AtomicUsize,
    attaches: UnboundedSender<Attached>,
}

impl Connector for FakeConnector {
    type Terminals = FakeTerms;

    async fn open(&self, _host: &HostConfig) -> Result<Opened, OpenError> {
        std::future::pending().await
    }

    async fn open_terminals(&self, _host: &HostConfig) -> Result<FakeTerms, OpenError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = self.failures.lock().unwrap().pop_front() {
            return Err(e);
        }
        Ok(FakeTerms {
            attaches: self.attaches.clone(),
        })
    }
}

struct Harness {
    connector: Arc<FakeConnector>,
    cmds: UnboundedSender<TermCmd>,
    events: UnboundedReceiver<(String, HostEvent)>,
    attaches: UnboundedReceiver<Attached>,
    zellij: ZellijPaths,
}

fn start(zellij_cfg: Option<&str>, failures: Vec<OpenError>) -> Harness {
    let (attach_tx, attaches) = mpsc::unbounded_channel();
    let connector = Arc::new(FakeConnector {
        failures: Mutex::new(failures.into()),
        opens: AtomicUsize::new(0),
        attaches: attach_tx,
    });
    let (ev_tx, events) = mpsc::unbounded_channel();
    let (cmds, cmd_rx) = mpsc::unbounded_channel();
    let zellij = ZellijPaths::default();
    let cfg = HostConfig {
        id: "h".into(),
        kind: HostKind::Local,
        zellij: zellij_cfg.map(str::to_owned),
    };
    tokio::spawn(run_terms(
        cfg,
        connector.clone(),
        zellij.clone(),
        ev_tx,
        cmd_rx,
    ));
    Harness {
        connector,
        cmds,
        events,
        attaches,
        zellij,
    }
}

impl Harness {
    async fn open(&self, session: &str) -> OpenResult {
        let (reply, answer) = oneshot::channel();
        let spec = AttachSpec {
            session: session.into(),
            create: false,
            size: SIZE,
        };
        self.cmds.send(TermCmd::Open { spec, reply }).unwrap();
        answer.await.unwrap()
    }

    async fn attached(&mut self) -> Attached {
        timeout(Duration::from_secs(600), self.attaches.recv())
            .await
            .expect("attach")
            .unwrap()
    }

    /// Next terminal connection state.
    async fn state(&mut self) -> Option<ConnState> {
        loop {
            let (_, ev) = timeout(Duration::from_secs(600), self.events.recv())
                .await
                .expect("host event")
                .unwrap();
            if let HostEvent::Term(s) = ev {
                return s;
            }
        }
    }
}

async fn next(rx: &mut UnboundedReceiver<TermEvent>) -> Option<TermEvent> {
    timeout(Duration::from_secs(600), rx.recv())
        .await
        .expect("terminal event")
}

#[tokio::test(start_paused = true)]
async fn terminal_relays_both_ways_and_ends_on_exit() {
    let mut h = start(None, vec![]);
    let (id, mut rx) = h.open("work").await.unwrap();
    assert_eq!(h.state().await, Some(ConnState::Up));
    let mut pty = h.attached().await;
    assert_eq!(pty.spec.session, "work");
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));

    h.cmds
        .send(TermCmd::Input {
            id,
            data: b"ls\r".to_vec(),
        })
        .unwrap();
    assert_eq!(
        pty.ends.input.recv().await,
        Some(PtyIn::Data(b"ls\r".to_vec()))
    );
    let bigger = TermSize {
        cols: 120,
        rows: 40,
    };
    h.cmds.send(TermCmd::Resize { id, size: bigger }).unwrap();
    assert_eq!(pty.ends.input.recv().await, Some(PtyIn::Resize(bigger)));

    pty.ends.output.send(PtyOut::Data(b"hi".to_vec())).unwrap();
    assert_eq!(next(&mut rx).await, Some(TermEvent::Output(b"hi".to_vec())));
    pty.ends.output.send(PtyOut::Exit(Some(0))).unwrap();
    assert_eq!(next(&mut rx).await, Some(TermEvent::Exited(Some(0))));
    assert_eq!(
        next(&mut rx).await,
        None,
        "finished terminal ends its stream"
    );
    assert_eq!(h.state().await, None, "last terminal closes the connection");
}

#[tokio::test(start_paused = true)]
async fn zellij_from_config_wins_over_probe_report() {
    let mut h = start(Some("/cfg/zellij"), vec![]);
    h.zellij
        .lock()
        .unwrap()
        .insert("h".into(), "/probe/zellij".into());
    h.open("w").await.unwrap();
    assert_eq!(h.attached().await.zellij.as_deref(), Some("/cfg/zellij"));

    let mut h = start(None, vec![]);
    h.open("w").await.unwrap();
    assert_eq!(h.attached().await.zellij, None);
    h.zellij
        .lock()
        .unwrap()
        .insert("h".into(), "/probe/zellij".into());
    h.open("w2").await.unwrap();
    assert_eq!(h.attached().await.zellij.as_deref(), Some("/probe/zellij"));
}

#[tokio::test(start_paused = true)]
async fn lost_connection_detaches_then_reattaches_with_current_size() {
    let mut h = start(None, vec![]);
    let (id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    let first = h.attached().await;
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));
    let bigger = TermSize {
        cols: 132,
        rows: 50,
    };
    h.cmds.send(TermCmd::Resize { id, size: bigger }).unwrap();

    drop(first.ends.output); // output ends without Exit: connection lost
    match next(&mut rx).await {
        Some(TermEvent::Detached { reason }) => assert!(reason.contains("lost"), "{reason}"),
        other => panic!("{other:?}"),
    }
    match h.state().await {
        Some(ConnState::Retrying { retry_in, .. }) => {
            assert_eq!(retry_in, Duration::from_secs(1));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(h.state().await, Some(ConnState::Up));
    let second = h.attached().await;
    assert_eq!(second.spec.session, "work");
    assert_eq!(second.spec.size, bigger);
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));
    assert_eq!(h.connector.opens.load(Ordering::SeqCst), 2);

    // Output of the new PTY flows; the old one is ignored.
    second
        .ends
        .output
        .send(PtyOut::Data(b"again".to_vec()))
        .unwrap();
    assert_eq!(
        next(&mut rx).await,
        Some(TermEvent::Output(b"again".to_vec()))
    );
}

#[tokio::test(start_paused = true)]
async fn closing_the_last_terminal_detaches_and_closes_the_connection() {
    let mut h = start(None, vec![]);
    let (id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    let mut pty = h.attached().await;
    h.cmds.send(TermCmd::Close { id }).unwrap();
    assert_eq!(pty.ends.input.recv().await, Some(PtyIn::Close));
    assert_eq!(h.state().await, None);
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));
    assert_eq!(next(&mut rx).await, None);
}

#[tokio::test(start_paused = true)]
async fn failed_open_is_reported_to_the_caller() {
    let mut h = start(
        None,
        vec![OpenError::NeedsUser(Problem::Auth("denied".into()))],
    );
    let r = h.open("work").await;
    assert_eq!(
        r.err(),
        Some(TermError::Open(OpenError::NeedsUser(Problem::Auth(
            "denied".into()
        ))))
    );
    assert_eq!(h.state().await, None);
    assert!(h.open("work").await.is_ok(), "next open tries again");
}

#[tokio::test(start_paused = true)]
async fn reconnect_needing_the_user_waits_for_retry() {
    let mut h = start(None, vec![]);
    let (_id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    let first = h.attached().await;
    next(&mut rx).await;
    h.connector
        .failures
        .lock()
        .unwrap()
        .push_back(OpenError::NeedsUser(Problem::HostKeyRejected));
    drop(first.ends.output);
    assert!(matches!(h.state().await, Some(ConnState::Retrying { .. })));
    assert_eq!(
        h.state().await,
        Some(ConnState::NeedsUser(Problem::HostKeyRejected))
    );
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(
        h.connector.opens.load(Ordering::SeqCst),
        2,
        "no automatic retry"
    );
    h.cmds.send(TermCmd::Retry).unwrap();
    assert_eq!(h.state().await, Some(ConnState::Up));
    h.attached().await;
}

#[tokio::test(start_paused = true)]
async fn opening_a_terminal_while_detached_reattaches_the_others() {
    let mut h = start(None, vec![]);
    let (_id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    let first = h.attached().await;
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));

    drop(first.ends.output);
    assert!(matches!(
        next(&mut rx).await,
        Some(TermEvent::Detached { .. })
    ));
    // Before the backoff fires: opening reconnects and reattaches all.
    let (_id2, _rx2) = h.open("logs").await.unwrap();
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));
    let a = h.attached().await;
    let b = h.attached().await;
    let mut sessions = vec![a.spec.session, b.spec.session];
    sessions.sort();
    assert_eq!(sessions, vec!["logs", "work"]);
}
