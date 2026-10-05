//! `SshSession::open_pty` against an in-process russh server that records
//! channel requests, echoes input, reports resizes and can exit or drop.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mai_core::pty::{PtyIn, PtyIo, PtyOut, TermSize};
use mai_core::ssh::auth::{KbdPrompt, Prompter, Secret, SecretStore};
use mai_core::ssh::client::{ConnectOptions, SshSession, connect};
use mai_core::ssh::config::HostSpec;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;
use tokio::time::timeout;

type Log = Arc<Mutex<Vec<String>>>;

#[derive(Clone)]
struct PtyServer {
    accept_env: bool,
    log: Log,
}

impl PtyServer {
    fn note(&self, s: String) {
        self.log.lock().unwrap().push(s);
    }
}

impl server::Handler for PtyServer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.note(format!("pty {term} {cols}x{rows}"));
        session.channel_success(channel)
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        value: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.accept_env {
            self.note(format!("env {name}={value}"));
            session.channel_success(channel)
        } else {
            session.channel_failure(channel)
        }
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.note(format!("exec {}", String::from_utf8_lossy(data)));
        session.channel_success(channel)?;
        if data == b"two-streams" {
            session.data(channel, b"out-1\n".to_vec())?;
            session.extended_data(channel, 1, b"warn: first\nerror: bad rules\n".to_vec())?;
            session.data(channel, b"out-2\n".to_vec())?;
            session.exit_status_request(channel, 1)?;
            session.eof(channel)?;
            return session.close(channel);
        }
        session.data(channel, b"ready\r\n".to_vec())
    }

    /// `exit` ends with status 5; `drop` closes without a status (lost
    /// connection); anything else is echoed.
    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if data.starts_with(b"exit") {
            session.exit_status_request(channel, 5)?;
            session.eof(channel)?;
            session.close(channel)
        } else if data.starts_with(b"drop") {
            session.close(channel)
        } else {
            session.data(channel, data.to_vec())
        }
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, format!("size={cols}x{rows}\r\n").into_bytes())
    }
}

async fn start(server: PtyServer) -> u16 {
    let key = PrivateKey::random(&mut UnwrapErr(SysRng), Algorithm::Ed25519).unwrap();
    let config = Arc::new(server::Config {
        keys: vec![key],
        methods: MethodSet::from(&[MethodKind::None][..]),
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (config, server) = (config.clone(), server.clone());
            tokio::spawn(async move {
                let _ = server::run_stream(config, stream, server).await;
            });
        }
    });
    port
}

/// Trusts every host key; never has secrets.
struct Trusting;

impl Prompter for Trusting {
    async fn confirm_host_key(&self, _h: &str, _p: u16, _f: &str) -> bool {
        true
    }
    async fn password(&self, _u: &str, _h: &str) -> Option<Secret> {
        None
    }
    async fn passphrase(&self, _k: &Path) -> Option<Secret> {
        None
    }
    async fn keyboard_interactive(
        &self,
        _n: &str,
        _i: &str,
        _p: &[KbdPrompt],
    ) -> Option<Vec<String>> {
        None
    }
}

struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _key: &str) -> Option<String> {
        None
    }
    fn set(&self, _key: &str, _value: &str) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, _key: &str) -> Result<(), String> {
        Ok(())
    }
}

async fn session(accept_env: bool) -> (SshSession<Trusting>, Log, tempfile::TempDir) {
    let log = Log::default();
    let port = start(PtyServer {
        accept_env,
        log: log.clone(),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let learn_to = dir.path().join("known_hosts");
    let opts = ConnectOptions {
        known_hosts: vec![learn_to.clone()],
        learn_to,
        timeout: Duration::from_secs(10),
    };
    let spec = HostSpec {
        alias: "pty".into(),
        host: "127.0.0.1".into(),
        port,
        user: "tester".into(),
        identity_files: vec![],
        jumps: vec![],
    };
    let s = connect(&spec, &opts, Arc::new(Trusting), &NoSecrets)
        .await
        .expect("connect");
    (s, log, dir)
}

const ENV: [(&str, &str); 2] = [("LANG", "en_US.UTF-8"), ("LC_CTYPE", "en_US.UTF-8")];

fn command(accepted: bool) -> String {
    if accepted {
        "zellij attach work".into()
    } else {
        "LANG=en_US.UTF-8 LC_CTYPE=en_US.UTF-8 zellij attach work".into()
    }
}

/// Read output until it contains `needle`.
async fn read_until(io: &mut PtyIo, needle: &str) {
    let mut seen = String::new();
    while !seen.contains(needle) {
        match timeout(Duration::from_secs(10), io.output.recv()).await {
            Ok(Some(PtyOut::Data(d))) => seen.push_str(&String::from_utf8_lossy(&d)),
            other => panic!("waiting for {needle:?}, got {other:?} after {seen:?}"),
        }
    }
}

/// The next non-data event, or `None` when output ended.
async fn end(io: &mut PtyIo) -> Option<PtyOut> {
    loop {
        match timeout(Duration::from_secs(10), io.output.recv()).await {
            Ok(Some(PtyOut::Data(_))) => continue,
            Ok(other) => return other,
            Err(_) => panic!("pty did not end"),
        }
    }
}

#[tokio::test]
async fn pty_carries_input_output_resize_and_exit() {
    let (s, log, _dir) = session(true).await;
    let size = TermSize {
        cols: 100,
        rows: 30,
    };
    let mut io = s.open_pty(size, &ENV, command).await.unwrap();
    read_until(&mut io, "ready").await;
    io.input.send(PtyIn::Data(b"hello".to_vec())).unwrap();
    read_until(&mut io, "hello").await;
    io.input
        .send(PtyIn::Resize(TermSize {
            cols: 120,
            rows: 40,
        }))
        .unwrap();
    read_until(&mut io, "size=120x40").await;
    io.input.send(PtyIn::Data(b"exit".to_vec())).unwrap();
    assert_eq!(end(&mut io).await, Some(PtyOut::Exit(Some(5))));
    assert_eq!(
        log.lock().unwrap().clone(),
        vec![
            "pty xterm-256color 100x30",
            "env LANG=en_US.UTF-8",
            "env LC_CTYPE=en_US.UTF-8",
            "exec zellij attach work",
        ]
    );
}

#[tokio::test]
async fn refused_env_falls_back_to_command_prefix() {
    let (s, log, _dir) = session(false).await;
    let size = TermSize { cols: 80, rows: 24 };
    let mut io = s.open_pty(size, &ENV, command).await.unwrap();
    read_until(&mut io, "ready").await;
    assert_eq!(
        log.lock().unwrap().last().cloned(),
        Some("exec LANG=en_US.UTF-8 LC_CTYPE=en_US.UTF-8 zellij attach work".to_owned())
    );
}

#[tokio::test]
async fn channel_closed_without_exit_status_is_a_lost_connection() {
    let (s, _log, _dir) = session(true).await;
    let size = TermSize { cols: 80, rows: 24 };
    let mut io = s.open_pty(size, &ENV, command).await.unwrap();
    read_until(&mut io, "ready").await;
    io.input.send(PtyIn::Data(b"drop".to_vec())).unwrap();
    assert_eq!(end(&mut io).await, None, "no Exit: the connection was lost");
}

#[tokio::test]
async fn exec_split_separates_stdout_from_stderr() {
    use mai_core::stderr::StderrTail;
    use tokio::io::AsyncReadExt;

    let (s, _log, _dir) = session(true).await;
    let tail = Arc::new(StderrTail::default());
    let (mut out, _stdin) = s
        .open_exec_split("two-streams", &[], tail.clone())
        .await
        .unwrap();
    let mut text = String::new();
    timeout(Duration::from_secs(10), out.read_to_string(&mut text))
        .await
        .expect("stdout ends")
        .unwrap();
    assert_eq!(text, "out-1\nout-2\n");
    assert_eq!(
        tail.summary().await.as_deref(),
        Some("warn: first | error: bad rules")
    );
}
