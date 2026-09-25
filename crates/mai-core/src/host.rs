//! One task per monitored host keeps its probe connection alive: connect,
//! deploy, start `serve`, relay messages, and reconnect with backoff.
//! Because only this task connects and deploys to its host, those steps
//! never run concurrently for one host.

use std::any::Any;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mai_protocol::{AppMsg, ProbeMsg};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

use crate::deploy::HookResult;
use crate::link::{HelloInfo, LinkError, ProbeIo, ProbeLink};

/// How the app reaches a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKind {
    /// This machine: the probe runs as a child process.
    Local,
    /// An `ssh` target: a `~/.ssh/config` alias or `[user@]host[:port]`.
    Ssh { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfig {
    /// Stable id, unique among hosts (used in `AgentKey::host_id`).
    pub id: String,
    pub kind: HostKind,
    /// zellij binary on the host when it is not found automatically.
    pub zellij: Option<String>,
}

/// Why a connection waits for the user instead of retrying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// No authentication method succeeded.
    Auth(String),
    /// The user declined the host key.
    HostKeyRejected,
    /// The host key differs from a recorded one.
    HostKeyChanged { file: PathBuf, line: usize },
    /// The probe could not be put on the host.
    Deploy(String),
    /// The host's configuration cannot be used (e.g. ssh config error).
    Config(String),
    /// The deployed probe speaks another protocol version.
    Protocol { probe: u32, app: u32 },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(m) => write!(f, "authentication failed: {m}"),
            Self::HostKeyRejected => f.write_str("host key not accepted"),
            Self::HostKeyChanged { file, line } => write!(
                f,
                "HOST KEY CHANGED (recorded in {} line {line})",
                file.display()
            ),
            Self::Deploy(m) => write!(f, "probe deployment failed: {m}"),
            Self::Config(m) => write!(f, "configuration error: {m}"),
            Self::Protocol { probe, app } => {
                write!(
                    f,
                    "probe protocol {probe} does not match app protocol {app}"
                )
            }
        }
    }
}

/// Why `Connector::open` failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Transient (network, timeout): retried with backoff.
    Retry(String),
    /// Needs the user: not retried until `HostCommand::Retry`.
    NeedsUser(Problem),
}

/// A started probe.
pub struct Opened {
    pub io: ProbeIo,
    /// Result of `install-hooks`. A failure only degrades detection to
    /// screen scraping, so it does not fail the connection.
    pub hooks: Result<Vec<HookResult>, String>,
    /// Kept alive while the probe runs (SSH session, child process).
    pub keep: Box<dyn Any + Send>,
}

/// Connects to a host, deploys the probe and starts `serve`.
pub trait Connector: Send + Sync + 'static {
    fn open(&self, host: &HostConfig) -> impl Future<Output = Result<Opened, OpenError>> + Send;
}

/// State of one connection of a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Up,
    /// Down; the next attempt starts after `retry_in`.
    Retrying {
        retry_in: Duration,
        reason: String,
    },
    /// Down until the user acts (`HostCommand::Retry`).
    NeedsUser(Problem),
}

/// Overall host state shown in the UI (design 3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostState {
    Connecting,
    Online,
    /// Only one of the two connections is up.
    Degraded,
    Offline,
    AuthRequired,
}

fn needs_auth(c: &ConnState) -> bool {
    matches!(
        c,
        ConnState::NeedsUser(
            Problem::Auth(_) | Problem::HostKeyRejected | Problem::HostKeyChanged { .. }
        )
    )
}

/// Host state from its probe connection and, once the terminal
/// connection exists, the terminal connection (`None` until then).
pub fn host_state(probe: &ConnState, term: Option<&ConnState>) -> HostState {
    let conns: Vec<&ConnState> = std::iter::once(probe).chain(term).collect();
    let up = conns.iter().filter(|c| ***c == ConnState::Up).count();
    if up == conns.len() {
        HostState::Online
    } else if up > 0 {
        HostState::Degraded
    } else if conns.iter().any(|c| needs_auth(c)) {
        HostState::AuthRequired
    } else if conns.iter().any(|c| **c == ConnState::Connecting) {
        HostState::Connecting
    } else {
        HostState::Offline
    }
}

/// Reconnect delays: 1 s doubling to 60 s (design 3.4).
#[derive(Debug, Clone)]
pub struct Backoff {
    next: Duration,
}

impl Backoff {
    pub const FIRST: Duration = Duration::from_secs(1);
    pub const MAX: Duration = Duration::from_secs(60);

    /// The delay to wait now; the following one doubles.
    pub fn next_delay(&mut self) -> Duration {
        let d = self.next;
        self.next = (d * 2).min(Self::MAX);
        d
    }

    pub fn reset(&mut self) {
        self.next = Self::FIRST;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self { next: Self::FIRST }
    }
}

/// A probe that stayed up this long resets the backoff when it drops,
/// so a probe that dies right after starting keeps backing off.
pub const STABLE_AFTER: Duration = Duration::from_secs(30);

/// Sent by a host task to the monitor.
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    Probe(ConnState),
    Hello(HelloInfo),
    Hooks(Result<Vec<HookResult>, String>),
    Msg(ProbeMsg),
    /// A command could not be delivered (probe not connected).
    Dropped(AppMsg),
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostCommand {
    /// Connect now: skips a backoff wait or leaves `NeedsUser`.
    Retry,
    Send(AppMsg),
    Stop,
}

type Events = UnboundedSender<(String, HostEvent)>;

struct Host<C> {
    cfg: HostConfig,
    connector: Arc<C>,
    events: Events,
    cmds: UnboundedReceiver<HostCommand>,
}

/// What a command received while waiting means for the wait.
enum Wake {
    Retry,
    Stop,
}

impl<C: Connector> Host<C> {
    fn emit(&self, ev: HostEvent) {
        let _ = self.events.send((self.cfg.id.clone(), ev));
    }

    /// Handle a command that arrived while no probe is running.
    fn idle_command(&self, cmd: Option<HostCommand>) -> Option<Wake> {
        match cmd {
            None | Some(HostCommand::Stop) => Some(Wake::Stop),
            Some(HostCommand::Retry) => Some(Wake::Retry),
            Some(HostCommand::Send(m)) => {
                self.emit(HostEvent::Dropped(m));
                None
            }
        }
    }

    /// Run `fut` while answering commands; `None` if told to stop.
    /// `Retry` is ignored: an attempt is already under way.
    async fn busy<F: Future>(&mut self, fut: F) -> Option<F::Output> {
        tokio::pin!(fut);
        loop {
            tokio::select! {
                out = &mut fut => return Some(out),
                cmd = self.cmds.recv() => {
                    if let Some(Wake::Stop) = self.idle_command(cmd) {
                        return None;
                    }
                }
            }
        }
    }

    /// Wait `delay` (forever if `None`) or until `Retry`; false on stop.
    async fn wait(&mut self, delay: Option<Duration>) -> bool {
        // Without a delay the sleep branch is disabled; its length is moot.
        let sleep = tokio::time::sleep(delay.unwrap_or(Duration::ZERO));
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep, if delay.is_some() => return true,
                cmd = self.cmds.recv() => match self.idle_command(cmd) {
                    Some(Wake::Retry) => return true,
                    Some(Wake::Stop) => return false,
                    None => {}
                },
            }
        }
    }

    /// Relay messages until the link fails (`Some(reason)`) or the task
    /// is told to stop (`None`). Hook events are acknowledged once they
    /// have been handed to the monitor.
    async fn serve(&mut self, link: &mut ProbeLink) -> Option<LinkError> {
        loop {
            tokio::select! {
                msg = link.recv() => {
                    let msg = match msg {
                        Ok(m) => m,
                        Err(e) => return Some(e),
                    };
                    let ack = match &msg {
                        ProbeMsg::AgentEvent(ev) => ev.spool_offset,
                        _ => None,
                    };
                    self.emit(HostEvent::Msg(msg));
                    if let Some(spool_offset) = ack
                        && let Err(e) = link.send(&AppMsg::Ack { spool_offset }).await
                    {
                        return Some(e);
                    }
                }
                cmd = self.cmds.recv() => match cmd {
                    None | Some(HostCommand::Stop) => return None,
                    Some(HostCommand::Retry) => {}
                    Some(HostCommand::Send(m)) => {
                        if let Err(e) = link.send(&m).await {
                            self.emit(HostEvent::Dropped(m));
                            return Some(e);
                        }
                    }
                },
            }
        }
    }

    async fn run(mut self) {
        let mut backoff = Backoff::default();
        loop {
            self.emit(HostEvent::Probe(ConnState::Connecting));
            let connector = self.connector.clone();
            let cfg = self.cfg.clone();
            let Some(opened) = self.busy(connector.open(&cfg)).await else {
                return;
            };
            let reason = match opened {
                Err(OpenError::NeedsUser(p)) => {
                    self.emit(HostEvent::Probe(ConnState::NeedsUser(p)));
                    if !self.wait(None).await {
                        return;
                    }
                    backoff.reset();
                    continue;
                }
                Err(OpenError::Retry(reason)) => reason,
                Ok(opened) => {
                    let mut link = ProbeLink::new(opened.io);
                    let Some(hello) = self.busy(link.hello()).await else {
                        return;
                    };
                    match hello {
                        Err(LinkError::Protocol { probe, app }) => {
                            let p = Problem::Protocol { probe, app };
                            self.emit(HostEvent::Probe(ConnState::NeedsUser(p)));
                            if !self.wait(None).await {
                                return;
                            }
                            continue;
                        }
                        Err(e) => e.to_string(),
                        Ok(hello) => {
                            self.emit(HostEvent::Hello(hello));
                            self.emit(HostEvent::Hooks(opened.hooks));
                            self.emit(HostEvent::Probe(ConnState::Up));
                            let up_since = Instant::now();
                            let Some(e) = self.serve(&mut link).await else {
                                return;
                            };
                            if up_since.elapsed() >= STABLE_AFTER {
                                backoff.reset();
                            }
                            e.to_string()
                        }
                    }
                }
            };
            let retry_in = backoff.next_delay();
            self.emit(HostEvent::Probe(ConnState::Retrying { retry_in, reason }));
            if !self.wait(Some(retry_in)).await {
                return;
            }
        }
    }
}

/// Run the probe connection of `cfg` until `HostCommand::Stop` or until
/// the command channel closes.
pub async fn run_host<C: Connector>(
    cfg: HostConfig,
    connector: Arc<C>,
    events: Events,
    cmds: UnboundedReceiver<HostCommand>,
) {
    Host {
        cfg,
        connector,
        events,
        cmds,
    }
    .run()
    .await
}
