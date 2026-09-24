//! SSH client tests against an in-process russh server (no network, no
//! external sshd). Keys are generated per test run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mai_core::ssh::auth::{KbdPrompt, Prompter, Secret, SecretStore, passphrase_key, password_key};
use mai_core::ssh::client::{ConnectOptions, SshError, connect};
use mai_core::ssh::config::HostSpec;
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use ssh_key::LineEnding;
use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;

const PASSWORD: &str = "s3cret";
const OTP: &str = "123456";

fn random_key() -> PrivateKey {
    PrivateKey::random(&mut UnwrapErr(SysRng), Algorithm::Ed25519).unwrap()
}

/// What the test server accepts.
#[derive(Clone)]
struct ServerCfg {
    methods: Vec<MethodKind>,
    client_key: Option<PublicKey>,
}

#[derive(Clone)]
struct TestServer(ServerCfg);

impl TestServer {
    /// Reject but keep every method available, like OpenSSH (which allows
    /// several attempts); russh's default reject drops the method.
    fn retry(&self) -> Auth {
        Auth::Reject {
            proceed_with_methods: Some(MethodSet::from(&self.0.methods[..])),
            partial_success: false,
        }
    }
}

impl server::Handler for TestServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == PASSWORD {
            Auth::Accept
        } else {
            self.retry()
        })
    }

    async fn auth_publickey(&mut self, _user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        let ok = self
            .0
            .client_key
            .as_ref()
            .is_some_and(|k| k.key_data() == key.key_data());
        Ok(if ok { Auth::Accept } else { self.retry() })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _user: &str,
        _submethods: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        let Some(mut response) = response else {
            return Ok(Auth::Partial {
                name: "otp".into(),
                instructions: "enter code".into(),
                prompts: vec![("Code: ".into(), false)].into(),
            });
        };
        let answer = response
            .next()
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        Ok(if answer.as_deref() == Some(OTP) {
            Auth::Accept
        } else {
            self.retry()
        })
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

    /// Echo the command on stdout, "e" on stderr, exit status 7.
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, data.to_vec())?;
        session.extended_data(channel, 1, b"e".to_vec())?;
        session.exit_status_request(channel, 7)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }
}

/// Start a server on 127.0.0.1:<random>; returns its port and host key.
async fn start(cfg: ServerCfg) -> (u16, PublicKey) {
    start_after(cfg, Duration::ZERO).await
}

/// Like `start`, but each accepted connection waits `delay` before the SSH
/// handshake (a slow server).
async fn start_after(cfg: ServerCfg, delay: Duration) -> (u16, PublicKey) {
    let host_key = random_key();
    let public = host_key.public_key().clone();
    let config = Arc::new(server::Config {
        keys: vec![host_key],
        methods: MethodSet::from(&cfg.methods[..]),
        auth_rejection_time: Duration::from_millis(1),
        auth_rejection_time_initial: Some(Duration::from_millis(1)),
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (config, cfg) = (config.clone(), cfg.clone());
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let _ = server::run_stream(config, stream, TestServer(cfg)).await;
            });
        }
    });
    (port, public)
}

#[derive(Default)]
struct Scripted {
    trust: bool,
    /// How long the user "thinks" before answering the host-key prompt.
    confirm_delay: Duration,
    password: Option<Secret>,
    passphrase: Option<Secret>,
    kbd: Option<Vec<String>>,
    calls: Mutex<Vec<String>>,
}

impl Scripted {
    fn log(&self, what: &str) {
        self.calls.lock().unwrap().push(what.to_owned());
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Prompter for Scripted {
    fn confirm_host_key(&self, _h: &str, _p: u16, _fp: &str) -> impl Future<Output = bool> + Send {
        self.log("host_key");
        let (t, delay) = (self.trust, self.confirm_delay);
        async move {
            tokio::time::sleep(delay).await;
            t
        }
    }
    fn password(&self, _u: &str, _h: &str) -> impl Future<Output = Option<Secret>> + Send {
        self.log("password");
        let v = self.password.clone();
        async move { v }
    }
    fn passphrase(&self, _k: &Path) -> impl Future<Output = Option<Secret>> + Send {
        self.log("passphrase");
        let v = self.passphrase.clone();
        async move { v }
    }
    fn keyboard_interactive(
        &self,
        _n: &str,
        _i: &str,
        prompts: &[KbdPrompt],
    ) -> impl Future<Output = Option<Vec<String>>> + Send {
        self.log(&format!("kbd:{}", prompts.len()));
        let v = self.kbd.clone();
        async move { v }
    }
}

#[derive(Default)]
struct MemStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemStore {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

fn spec(port: u16, identity_files: Vec<PathBuf>) -> HostSpec {
    HostSpec {
        alias: "test".into(),
        host: "127.0.0.1".into(),
        port,
        user: "tester".into(),
        identity_files,
        jumps: vec![],
    }
}

fn opts(dir: &Path) -> ConnectOptions {
    let learn_to = dir.join("known_hosts");
    ConnectOptions {
        known_hosts: vec![learn_to.clone()],
        learn_to,
        timeout: Duration::from_secs(10),
    }
}

fn secret(v: &str, remember: bool) -> Option<Secret> {
    Some(Secret {
        value: v.into(),
        remember,
    })
}

#[tokio::test]
async fn prompted_password_is_remembered_and_host_key_learned() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, true),
        ..Default::default()
    });
    let store = MemStore::default();
    let s = connect(&spec(port, vec![]), &opts(dir.path()), p.clone(), &store)
        .await
        .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "password"]);
    assert_eq!(store.get(&password_key("test")).as_deref(), Some(PASSWORD));
    assert!(
        std::fs::read_to_string(dir.path().join("known_hosts"))
            .unwrap()
            .contains("127.0.0.1")
    );
    s.close().await;

    // Second connection: key known, password from the store, no prompts.
    let p2 = Arc::new(Scripted::default());
    connect(&spec(port, vec![]), &opts(dir.path()), p2.clone(), &store)
        .await
        .unwrap();
    assert!(p2.calls().is_empty(), "{:?}", p2.calls());
}

/// `learn_to` need not be listed in `known_hosts` for a key already
/// learned there to be found again: the checker always consults it too.
#[tokio::test]
async fn learn_to_is_checked_even_if_absent_from_known_hosts() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let o = ConnectOptions {
        known_hosts: vec![dir.path().join("does_not_exist")],
        learn_to: dir.path().join("app_known_hosts"),
        timeout: Duration::from_secs(10),
    };
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    connect(&spec(port, vec![]), &o, p.clone(), &MemStore::default())
        .await
        .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "password"]);

    // Second connection: `known_hosts` still does not list `learn_to`, but
    // the key was written there, so it must be found without a prompt.
    let p2 = Arc::new(Scripted {
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    connect(&spec(port, vec![]), &o, p2.clone(), &MemStore::default())
        .await
        .unwrap();
    assert_eq!(p2.calls(), vec!["password"]);
}

#[tokio::test]
async fn wrong_stored_password_is_dropped_then_prompted() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let store = MemStore::default();
    store.set(&password_key("test"), "stale").unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    connect(&spec(port, vec![]), &opts(dir.path()), p.clone(), &store)
        .await
        .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "password"]);
    assert_eq!(store.get(&password_key("test")), None);
}

#[tokio::test]
async fn keyboard_interactive_answers_prompts() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::KeyboardInteractive],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        kbd: Some(vec![OTP.into()]),
        ..Default::default()
    });
    connect(
        &spec(port, vec![]),
        &opts(dir.path()),
        p.clone(),
        &MemStore::default(),
    )
    .await
    .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "kbd:1"]);
}

#[tokio::test]
async fn private_key_file_needs_no_prompts() {
    let client = random_key();
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::PublicKey],
        client_key: Some(client.public_key().clone()),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("id_test");
    client
        .write_openssh_file(&key_file, LineEnding::LF)
        .unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        ..Default::default()
    });
    let files = vec![dir.path().join("missing_key"), key_file];
    connect(
        &spec(port, files),
        &opts(dir.path()),
        p.clone(),
        &MemStore::default(),
    )
    .await
    .unwrap();
    assert_eq!(p.calls(), vec!["host_key"]);
}

#[tokio::test]
async fn encrypted_key_asks_passphrase_once_and_remembers() {
    let client = random_key();
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::PublicKey],
        client_key: Some(client.public_key().clone()),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("id_enc");
    client
        .encrypt(&mut SysRng, "pw")
        .unwrap()
        .write_openssh_file(&key_file, LineEnding::LF)
        .unwrap();
    let store = MemStore::default();
    let p = Arc::new(Scripted {
        trust: true,
        passphrase: secret("pw", true),
        ..Default::default()
    });
    connect(
        &spec(port, vec![key_file.clone()]),
        &opts(dir.path()),
        p.clone(),
        &store,
    )
    .await
    .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "passphrase"]);
    assert_eq!(store.get(&passphrase_key(&key_file)).as_deref(), Some("pw"));
}

#[tokio::test]
async fn declined_host_key_aborts() {
    let (port, public) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: false,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    let err = connect(
        &spec(port, vec![]),
        &opts(dir.path()),
        p.clone(),
        &MemStore::default(),
    )
    .await
    .err()
    .unwrap();
    let SshError::HostKeyRejected { fingerprint, .. } = err else {
        panic!("unexpected {err:?}");
    };
    assert_eq!(fingerprint, mai_core::ssh::hostkey::fingerprint(&public));
    assert_eq!(p.calls(), vec!["host_key"]);
}

#[tokio::test]
async fn changed_host_key_is_refused_without_prompt() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let other = random_key();
    mai_core::ssh::hostkey::learn(
        &dir.path().join("known_hosts"),
        "127.0.0.1",
        port,
        other.public_key(),
    )
    .unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        ..Default::default()
    });
    let err = connect(
        &spec(port, vec![]),
        &opts(dir.path()),
        p.clone(),
        &MemStore::default(),
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(err, SshError::HostKeyChanged { .. }), "{err:?}");
    assert!(p.calls().is_empty());
}

#[tokio::test]
async fn all_methods_failing_is_an_auth_error() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        password: secret("nope", true),
        ..Default::default()
    });
    let store = MemStore::default();
    let err = connect(&spec(port, vec![]), &opts(dir.path()), p, &store)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, SshError::Auth(_)), "{err:?}");
    assert_eq!(
        store.get(&password_key("test")),
        None,
        "wrong password not remembered"
    );
}

#[tokio::test]
async fn exec_collects_stdout_stderr_and_status() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    let s = connect(
        &spec(port, vec![]),
        &opts(dir.path()),
        p,
        &MemStore::default(),
    )
    .await
    .unwrap();
    let out = s.exec("uname -sm").await.unwrap();
    assert_eq!(out.stdout_str(), "uname -sm");
    assert_eq!(out.stderr, b"e");
    assert_eq!(out.status, Some(7));
    assert!(!out.success());
}

#[tokio::test]
async fn unreachable_host_is_a_connect_error() {
    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let p = Arc::new(Scripted::default());
    let err = connect(
        &spec(port, vec![]),
        &opts(dir.path()),
        p,
        &MemStore::default(),
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(err, SshError::Connect(_)), "{err:?}");
}

/// The connect timeout must not count the time the user spends on the
/// host-key prompt.
#[tokio::test]
async fn slow_host_key_answer_does_not_time_out() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut o = opts(dir.path());
    o.timeout = Duration::from_millis(300);
    let p = Arc::new(Scripted {
        trust: true,
        confirm_delay: Duration::from_millis(900),
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    connect(&spec(port, vec![]), &o, p.clone(), &MemStore::default())
        .await
        .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "password"]);
}

/// A connection that timed out before the host-key prompt must never
/// prompt or record the key later (the russh session task is detached and
/// keeps running after `connect` gives up).
#[tokio::test]
async fn timed_out_connection_never_prompts_later() {
    let (port, _) = start_after(
        ServerCfg {
            methods: vec![MethodKind::Password],
            client_key: None,
        },
        Duration::from_millis(600),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut o = opts(dir.path());
    o.timeout = Duration::from_millis(200);
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    let err = connect(&spec(port, vec![]), &o, p.clone(), &MemStore::default())
        .await
        .err()
        .unwrap();
    assert!(
        matches!(err, SshError::Connect(ref m) if m.contains("timed out")),
        "{err:?}"
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(p.calls().is_empty(), "{:?}", p.calls());
    assert!(!dir.path().join("known_hosts").exists());
}
