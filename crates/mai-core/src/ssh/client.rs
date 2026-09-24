//! SSH sessions: connect (optionally through jump hosts), verify the host
//! key, authenticate, and run commands.
//!
//! Auth order (design 3.2): private key files (passphrase from the secret
//! store or prompt), ssh-agent, remembered password, prompted password,
//! keyboard-interactive. Methods the server does not offer are skipped.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, MethodKind, MethodSet};
use tokio::io::{AsyncRead, AsyncWrite};

use super::auth::{KbdPrompt, Prompter, SecretStore, passphrase_key, password_key};
use super::config::HostSpec;
use super::hostkey::{self, HostKeyStatus};

/// Where host keys are checked and learned, and how long to wait.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Checked in order (e.g. `~/.ssh/known_hosts`, the app's own file).
    pub known_hosts: Vec<PathBuf>,
    /// Keys the user confirms are appended here.
    pub learn_to: PathBuf,
    /// TCP connect + key exchange timeout per hop.
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshError {
    Connect(String),
    /// The user declined an unknown host key.
    HostKeyRejected {
        host: String,
        port: u16,
        fingerprint: String,
    },
    /// known_hosts has a different key for this host: never connect.
    HostKeyChanged {
        host: String,
        port: u16,
        file: PathBuf,
        line: usize,
    },
    Auth(String),
    Channel(String),
}

impl fmt::Display for SshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(m) => write!(f, "connect: {m}"),
            Self::HostKeyRejected {
                host,
                port,
                fingerprint,
            } => {
                write!(f, "host key for {host}:{port} not trusted ({fingerprint})")
            }
            Self::HostKeyChanged {
                host,
                port,
                file,
                line,
            } => write!(
                f,
                "HOST KEY CHANGED for {host}:{port} (see {}:{line}); refusing to connect",
                file.display()
            ),
            Self::Auth(m) => write!(f, "authentication: {m}"),
            Self::Channel(m) => write!(f, "channel: {m}"),
        }
    }
}

impl std::error::Error for SshError {}

fn chan_err(e: impl fmt::Display) -> SshError {
    SshError::Channel(e.to_string())
}

fn auth_err(e: impl fmt::Display) -> SshError {
    SshError::Auth(e.to_string())
}

/// Why a host key was refused, filled in by the handler.
type Verdict = Arc<Mutex<Option<SshError>>>;

const IDLE: u8 = 0;
const PROMPTING: u8 = 1;
const ABANDONED: u8 = 2;

/// Coordination between the connect timeout and the host-key prompt.
/// `phase` moves IDLE -> PROMPTING -> IDLE (handler) or IDLE -> ABANDONED
/// (timeout); both use compare-and-swap, so a timed-out connection never
/// prompts or learns a key, and a prompt is never cut short by the timer.
#[derive(Default)]
struct HopState {
    phase: AtomicU8,
    /// Incremented when a prompt finishes; the timer then starts afresh.
    prompts_done: AtomicU64,
}

pub struct Checker<P> {
    host: String,
    port: u16,
    opts: ConnectOptions,
    prompter: Arc<P>,
    verdict: Verdict,
    state: Arc<HopState>,
}

impl<P: Prompter> client::Handler for Checker<P> {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            self.refuse(SshError::Connect(
                "host certificates are not supported".into(),
            ));
            return Ok(false);
        };
        match hostkey::check(&self.opts.known_hosts, &self.host, self.port, key) {
            HostKeyStatus::Known => Ok(true),
            HostKeyStatus::Changed { file, line } => {
                self.refuse(SshError::HostKeyChanged {
                    host: self.host.clone(),
                    port: self.port,
                    file,
                    line,
                });
                Ok(false)
            }
            HostKeyStatus::Unknown => {
                let claimed = self.state.phase.compare_exchange(
                    IDLE,
                    PROMPTING,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                if claimed.is_err() {
                    // The connection already timed out: do not prompt.
                    return Ok(false);
                }
                let fp = hostkey::fingerprint(key);
                let trusted = self
                    .prompter
                    .confirm_host_key(&self.host, self.port, &fp)
                    .await;
                // Decide and learn while still PROMPTING, so the timer
                // cannot abandon the connection in between.
                let accepted = if !trusted {
                    self.refuse(SshError::HostKeyRejected {
                        host: self.host.clone(),
                        port: self.port,
                        fingerprint: fp,
                    });
                    false
                } else if let Err(e) =
                    hostkey::learn(&self.opts.learn_to, &self.host, self.port, key)
                {
                    self.refuse(SshError::Connect(format!("cannot record host key: {e}")));
                    false
                } else {
                    true
                };
                self.state.prompts_done.fetch_add(1, Ordering::SeqCst);
                self.state.phase.store(IDLE, Ordering::SeqCst);
                Ok(accepted)
            }
        }
    }
}

impl<P> Checker<P> {
    fn refuse(&self, e: SshError) {
        if let Ok(mut v) = self.verdict.lock() {
            *v = Some(e);
        }
    }
}

/// Output of a finished remote command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub status: Option<u32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// An authenticated SSH connection. Keeps its jump-host connections alive.
pub struct SshSession<P: Prompter> {
    handle: Handle<Checker<P>>,
    _via: Option<Box<SshSession<P>>>,
}

fn client_config() -> Arc<client::Config> {
    Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    })
}

/// Connect to `spec`, hopping through `spec.jumps` (outermost first).
/// Jump hosts' own `ProxyJump` settings are not followed.
pub async fn connect<P: Prompter, S: SecretStore>(
    spec: &HostSpec,
    opts: &ConnectOptions,
    prompter: Arc<P>,
    secrets: &S,
) -> Result<SshSession<P>, SshError> {
    let mut via: Option<SshSession<P>> = None;
    for hop in spec.jumps.iter().chain(std::iter::once(spec)) {
        let session = connect_hop(hop, opts, prompter.clone(), secrets, via.take()).await?;
        via = Some(session);
    }
    via.ok_or_else(|| SshError::Connect("no hops".into()))
}

async fn connect_hop<P: Prompter, S: SecretStore>(
    hop: &HostSpec,
    opts: &ConnectOptions,
    prompter: Arc<P>,
    secrets: &S,
    via: Option<SshSession<P>>,
) -> Result<SshSession<P>, SshError> {
    let verdict: Verdict = Arc::default();
    let state = Arc::new(HopState::default());
    let checker = Checker {
        host: hop.host.clone(),
        port: hop.port,
        opts: opts.clone(),
        prompter: prompter.clone(),
        verdict: verdict.clone(),
        state: state.clone(),
    };
    let result = {
        let connecting = async {
            match &via {
                None => {
                    client::connect(client_config(), (hop.host.as_str(), hop.port), checker).await
                }
                Some(outer) => {
                    let ch = outer
                        .handle
                        .channel_open_direct_tcpip(
                            hop.host.clone(),
                            u32::from(hop.port),
                            "127.0.0.1",
                            0,
                        )
                        .await?;
                    client::connect_stream(client_config(), ch.into_stream(), checker).await
                }
            }
        };
        // The timeout covers TCP connect and key exchange only: it waits
        // while the host-key prompt is open, and starts a full period again
        // after a prompt finishes. Giving up marks the hop ABANDONED so the
        // detached russh session task can no longer prompt or learn a key.
        tokio::pin!(connecting);
        let mut seen = state.prompts_done.load(Ordering::SeqCst);
        loop {
            tokio::select! {
                r = &mut connecting => break r,
                _ = tokio::time::sleep(opts.timeout) => {
                    let done = state.prompts_done.load(Ordering::SeqCst);
                    if done != seen {
                        seen = done;
                        continue;
                    }
                    let gave_up = state.phase.compare_exchange(
                        IDLE,
                        ABANDONED,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    );
                    if gave_up.is_ok() {
                        return Err(SshError::Connect(format!(
                            "{}:{} timed out",
                            hop.host, hop.port
                        )));
                    }
                }
            }
        }
    };
    let mut handle = match result {
        Err(e) => {
            let refused = verdict.lock().ok().and_then(|mut v| v.take());
            return Err(refused
                .unwrap_or_else(|| SshError::Connect(format!("{}:{}: {e}", hop.host, hop.port))));
        }
        Ok(h) => h,
    };
    authenticate(&mut handle, hop, prompter.as_ref(), secrets).await?;
    Ok(SshSession {
        handle,
        _via: via.map(Box::new),
    })
}

fn offers(methods: &Option<MethodSet>, kind: MethodKind) -> bool {
    methods.as_ref().is_none_or(|m| m.contains(&kind))
}

/// Result of one authentication attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Fully authenticated.
    Done,
    /// This factor was accepted but the server wants more (e.g. 2FA).
    Partial,
    Failed,
}

/// Record the server's remaining methods and classify the result.
fn step(methods: &mut Option<MethodSet>, r: &client::AuthResult) -> Step {
    match r {
        client::AuthResult::Success => Step::Done,
        client::AuthResult::Failure {
            remaining_methods,
            partial_success,
        } => {
            *methods = Some(remaining_methods.clone());
            if *partial_success {
                Step::Partial
            } else {
                Step::Failed
            }
        }
    }
}

async fn authenticate<P: Prompter, S: SecretStore>(
    h: &mut Handle<Checker<P>>,
    spec: &HostSpec,
    prompter: &P,
    secrets: &S,
) -> Result<(), SshError> {
    let user = spec.user.as_str();
    let mut methods = match h.authenticate_none(user).await.map_err(auth_err)? {
        client::AuthResult::Success => return Ok(()),
        client::AuthResult::Failure {
            remaining_methods, ..
        } => Some(remaining_methods),
    };
    'keys: {
        if !offers(&methods, MethodKind::PublicKey) {
            break 'keys;
        }
        for path in spec.identity_files.iter().filter(|p| p.is_file()) {
            let Some(key) = load_key(path, prompter, secrets).await else {
                continue;
            };
            let hash = h
                .best_supported_rsa_hash()
                .await
                .map_err(auth_err)?
                .flatten();
            let r = h
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(auth_err)?;
            match step(&mut methods, &r) {
                Step::Done => return Ok(()),
                Step::Partial => break 'keys,
                Step::Failed => {}
            }
        }
        if try_agent(h, user).await? {
            return Ok(());
        }
    }

    if offers(&methods, MethodKind::Password) {
        let key = password_key(&spec.alias);
        let mut accepted = false;
        if let Some(pw) = secrets.get(&key) {
            let r = h.authenticate_password(user, pw).await.map_err(auth_err)?;
            match step(&mut methods, &r) {
                Step::Done => return Ok(()),
                Step::Partial => accepted = true,
                Step::Failed => {
                    let _ = secrets.delete(&key);
                }
            }
        }
        if !accepted
            && offers(&methods, MethodKind::Password)
            && let Some(s) = prompter.password(user, &spec.host).await
        {
            let r = h
                .authenticate_password(user, s.value.clone())
                .await
                .map_err(auth_err)?;
            let result = step(&mut methods, &r);
            if result != Step::Failed && s.remember {
                let _ = secrets.set(&key, &s.value);
            }
            if result == Step::Done {
                return Ok(());
            }
        }
    }

    if offers(&methods, MethodKind::KeyboardInteractive)
        && keyboard_interactive(h, spec, prompter).await?
    {
        return Ok(());
    }
    Err(SshError::Auth(format!(
        "no method succeeded for {user}@{}",
        spec.host
    )))
}

/// Load a private key; ask for (and optionally remember) its passphrase.
async fn load_key<P: Prompter, S: SecretStore>(
    path: &Path,
    prompter: &P,
    secrets: &S,
) -> Option<PrivateKey> {
    match russh::keys::load_secret_key(path, None) {
        Ok(k) => return Some(k),
        Err(russh::keys::Error::KeyIsEncrypted) => {}
        Err(_) => return None,
    }
    let key = passphrase_key(path);
    if let Some(pass) = secrets.get(&key) {
        if let Ok(k) = russh::keys::load_secret_key(path, Some(&pass)) {
            return Some(k);
        }
        let _ = secrets.delete(&key);
    }
    let s = prompter.passphrase(path).await?;
    let k = russh::keys::load_secret_key(path, Some(&s.value)).ok()?;
    if s.remember {
        let _ = secrets.set(&key, &s.value);
    }
    Some(k)
}

async fn agent_auth<P: Prompter, A>(
    h: &mut Handle<Checker<P>>,
    user: &str,
    mut agent: AgentClient<A>,
) -> Result<bool, SshError>
where
    A: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Ok(ids) = agent.request_identities().await else {
        return Ok(false);
    };
    for id in ids {
        let AgentIdentity::PublicKey { key, .. } = id else {
            continue;
        };
        let hash = h
            .best_supported_rsa_hash()
            .await
            .map_err(auth_err)?
            .flatten();
        if let Ok(r) = h
            .authenticate_publickey_with(user, key, hash, &mut agent)
            .await
            && r.success()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(unix)]
async fn try_agent<P: Prompter>(h: &mut Handle<Checker<P>>, user: &str) -> Result<bool, SshError> {
    match AgentClient::connect_env().await {
        Ok(agent) => agent_auth(h, user, agent).await,
        Err(_) => Ok(false),
    }
}

#[cfg(windows)]
async fn try_agent<P: Prompter>(h: &mut Handle<Checker<P>>, user: &str) -> Result<bool, SshError> {
    if let Ok(agent) = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await
        && agent_auth(h, user, agent).await?
    {
        return Ok(true);
    }
    match AgentClient::connect_pageant().await {
        Ok(agent) => agent_auth(h, user, agent).await,
        Err(_) => Ok(false),
    }
}

async fn keyboard_interactive<P: Prompter>(
    h: &mut Handle<Checker<P>>,
    spec: &HostSpec,
    prompter: &P,
) -> Result<bool, SshError> {
    let mut reply = h
        .authenticate_keyboard_interactive_start(spec.user.clone(), None)
        .await
        .map_err(auth_err)?;
    loop {
        match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let asks: Vec<KbdPrompt> = prompts
                    .iter()
                    .map(|p| KbdPrompt {
                        text: p.prompt.clone(),
                        echo: p.echo,
                    })
                    .collect();
                let answers = if asks.is_empty() {
                    Vec::new()
                } else {
                    match prompter
                        .keyboard_interactive(&name, &instructions, &asks)
                        .await
                    {
                        Some(a) => a,
                        None => return Ok(false),
                    }
                };
                reply = h
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(auth_err)?;
            }
        }
    }
}

impl<P: Prompter> SshSession<P> {
    /// Run `command` to completion, collecting stdout, stderr and status.
    pub async fn exec(&self, command: &str) -> Result<ExecOutput, SshError> {
        let mut ch = self.handle.channel_open_session().await.map_err(chan_err)?;
        ch.exec(true, command).await.map_err(chan_err)?;
        let mut out = ExecOutput::default();
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => out.stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => out.status = Some(exit_status),
                _ => {}
            }
        }
        Ok(out)
    }

    /// Start `command` and hand back its channel for streaming (stdin via
    /// `data`, stdout via `wait`). `env` is requested first; servers that
    /// do not accept a variable ignore it.
    pub async fn open_exec(
        &self,
        command: &str,
        env: &[(&str, &str)],
    ) -> Result<russh::Channel<client::Msg>, SshError> {
        let ch = self.handle.channel_open_session().await.map_err(chan_err)?;
        for (k, v) in env {
            ch.set_env(false, *k, *v).await.map_err(chan_err)?;
        }
        ch.exec(true, command).await.map_err(chan_err)?;
        Ok(ch)
    }

    /// Open an SFTP session. Relative paths are relative to the login
    /// directory (the home directory on OpenSSH servers).
    pub async fn sftp(&self) -> Result<russh_sftp::client::SftpSession, SshError> {
        let ch = self.handle.channel_open_session().await.map_err(chan_err)?;
        ch.request_subsystem(true, "sftp").await.map_err(chan_err)?;
        russh_sftp::client::SftpSession::new(ch.into_stream())
            .await
            .map_err(chan_err)
    }

    pub async fn close(self) {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }
}

// `step` is tested here because the in-process russh test server never
// sends `partial_success = true` (russh 0.63 resets it), so the 2FA path
// cannot be driven end-to-end in tests/ssh_session.rs.
#[cfg(test)]
mod tests {
    use super::*;

    fn failure(methods: &[MethodKind], partial: bool) -> client::AuthResult {
        client::AuthResult::Failure {
            remaining_methods: MethodSet::from(methods),
            partial_success: partial,
        }
    }

    #[test]
    fn step_classifies_success_partial_and_failure() {
        let mut methods = None;
        assert_eq!(step(&mut methods, &client::AuthResult::Success), Step::Done);
        assert_eq!(methods, None);

        let r = failure(&[MethodKind::KeyboardInteractive], true);
        assert_eq!(step(&mut methods, &r), Step::Partial);
        assert!(!offers(&methods, MethodKind::Password));
        assert!(offers(&methods, MethodKind::KeyboardInteractive));

        let r = failure(&[MethodKind::Password], false);
        assert_eq!(step(&mut methods, &r), Step::Failed);
        assert!(offers(&methods, MethodKind::Password));
    }

    #[test]
    fn unknown_method_set_offers_everything() {
        assert!(offers(&None, MethodKind::PublicKey));
        assert!(offers(&None, MethodKind::Password));
    }
}
