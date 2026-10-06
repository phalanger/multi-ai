//! Interactive terminals: `zellij attach <session>` in a PTY per open
//! terminal, over a host's terminal connection (TermConn, design 2.2).
//!
//! One task per host owns the terminal connection and all its PTYs. The
//! connection is opened with the first terminal and closed with the last.
//! When it is lost, every terminal is told it is detached, the task
//! reconnects with backoff and attaches each terminal again (zellij keeps
//! the session, so the user continues where they were). While connecting
//! or attaching, the task keeps handling input, resizes, closes and stop;
//! further opens wait their turn.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::deploy::{Remote, Shell};
use crate::host::{Backoff, ConnState, Connector, HostConfig, HostEvent, OpenError, STABLE_AFTER};
use crate::pty::{PtyIn, PtyIo, PtyOut, TermSize};

/// Locale requested for terminals (design 7): without it zellij sessions
/// created over SSH on macOS run in the C locale and CJK input breaks.
pub const LOCALE_ENV: [(&str, &str); 2] = [("LANG", "en_US.UTF-8"), ("LC_CTYPE", "en_US.UTF-8")];

/// A terminal whose channel keeps closing soon after attaching is given up
/// on after this many such closes in a row.
pub const MAX_QUICK_ENDS: u32 = 3;

/// Arguments after the zellij binary: `attach [--create] <session>`.
pub fn attach_argv(session: &str, create: bool) -> Vec<String> {
    let mut argv = vec!["attach".to_owned()];
    if create {
        argv.push("--create".to_owned());
    }
    argv.push(session.to_owned());
    argv
}

/// Command line that attaches to `session` on `remote`. `zellij` is the
/// binary when known (else `zellij` from PATH). When the server refused
/// the locale variables, a POSIX command sets them itself.
pub fn remote_attach_command(
    remote: &Remote,
    zellij: Option<&str>,
    session: &str,
    create: bool,
    env_accepted: bool,
) -> String {
    let cmd = remote.invoke(
        zellij.unwrap_or("zellij"),
        &remote.join_args(&attach_argv(session, create)),
    );
    if env_accepted || remote.shell != Shell::Posix {
        cmd
    } else {
        let prefix: Vec<String> = LOCALE_ENV.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{} {cmd}", prefix.join(" "))
    }
}

/// What to attach to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachSpec {
    pub session: String,
    /// Create the session if it does not exist (`attach --create`).
    pub create: bool,
    pub size: TermSize,
}

/// A host's terminal connection: starts `zellij attach` in PTYs.
pub trait TermTransport: Send + 'static {
    /// `zellij` is the binary to run when known.
    fn attach(
        &mut self,
        zellij: Option<&str>,
        spec: &AttachSpec,
    ) -> impl Future<Output = Result<PtyIo, OpenError>> + Send;

    /// Whether the connection itself is still up. When one terminal's
    /// output ends without an exit status, this tells a closed channel
    /// (reattach that terminal) from a lost connection (reattach all).
    fn alive(&self) -> bool;
}

/// Sent to the owner of a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    Output(Vec<u8>),
    /// Attached (again): zellij redraws the whole screen.
    Attached,
    /// The connection (or this terminal's channel) was lost; the terminal
    /// is reattached when possible.
    Detached {
        reason: String,
    },
    /// `zellij attach` exited (the user detached or quit the session), or
    /// its channel kept closing right after attaching. The terminal is
    /// finished.
    Exited(Option<u32>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermError {
    /// No host with this id.
    NoHost,
    /// The session name cannot be passed to zellij (empty, or starts
    /// with `-` and would be taken for an option).
    BadSession(String),
    /// The terminal connection or `zellij attach` could not be started.
    Open(OpenError),
    /// The host was removed while opening.
    Stopped,
}

impl fmt::Display for TermError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHost => f.write_str("no such host"),
            Self::BadSession(name) => write!(
                f,
                "invalid session name {name:?}: it must not be empty or start with '-'"
            ),
            Self::Open(OpenError::Retry(m)) => write!(f, "terminal connection failed: {m}"),
            Self::Open(OpenError::NeedsUser(p)) => write!(f, "{p}"),
            Self::Stopped => f.write_str("host removed"),
        }
    }
}

impl std::error::Error for TermError {}

/// Session names zellij would misread are refused up front.
pub fn check_session(name: &str) -> Result<(), TermError> {
    if name.is_empty() || name.starts_with('-') {
        return Err(TermError::BadSession(name.to_owned()));
    }
    Ok(())
}

pub type OpenResult = Result<(u64, UnboundedReceiver<TermEvent>), TermError>;

pub enum TermCmd {
    Open {
        spec: AttachSpec,
        reply: oneshot::Sender<OpenResult>,
    },
    Input {
        id: u64,
        data: Vec<u8>,
    },
    Resize {
        id: u64,
        size: TermSize,
    },
    Close {
        id: u64,
    },
    /// Reconnect now (after a problem that needs the user, or to skip a
    /// backoff wait).
    Retry,
    Stop,
}

/// zellij binaries reported by each host's probe (`Hello`), by host id.
pub type ZellijPaths = Arc<Mutex<HashMap<String, String>>>;

struct Slot {
    spec: AttachSpec,
    events: UnboundedSender<TermEvent>,
    /// `None` while detached.
    pty: Option<UnboundedSender<PtyIn>>,
    /// Bumped on every attach, so output of an older PTY is ignored.
    generation: u64,
    attached_at: Instant,
    /// Channel closes in a row that came soon after attaching.
    quick_ends: u32,
}

/// Output of one PTY, tagged with its terminal and attach generation;
/// `None` means the output ended.
type Tagged = (u64, u64, Option<PtyOut>);

enum Link<T> {
    /// No terminals, no connection.
    Idle,
    Up(T),
    /// Lent to an attach that is under way.
    Busy,
    /// Lost; reconnect at `at` (or on `Retry` if `None`). `error` is why,
    /// for terminals opened meanwhile.
    Down {
        at: Option<Instant>,
        error: OpenError,
    },
}

struct Terms<C: Connector> {
    cfg: HostConfig,
    connector: Arc<C>,
    zellij: ZellijPaths,
    events: UnboundedSender<(String, HostEvent)>,
    cmds: UnboundedReceiver<TermCmd>,
    link: Link<C::Terminals>,
    slots: HashMap<u64, Slot>,
    /// Opens that arrived while connecting or attaching.
    pending: VecDeque<(AttachSpec, oneshot::Sender<OpenResult>)>,
    next_id: u64,
    backoff: Backoff,
    /// When the current connection came up.
    up_since: Option<Instant>,
    out_tx: UnboundedSender<Tagged>,
}

fn reason_of(e: OpenError) -> String {
    match e {
        OpenError::Retry(m) => m,
        OpenError::NeedsUser(p) => p.to_string(),
    }
}

impl<C: Connector> Terms<C> {
    fn emit(&self, state: Option<ConnState>) {
        let _ = self
            .events
            .send((self.cfg.id.clone(), HostEvent::Term(state)));
    }

    fn zellij_path(&self) -> Option<String> {
        self.cfg.zellij.clone().or_else(|| {
            let paths = self.zellij.lock().unwrap_or_else(|p| p.into_inner());
            paths.get(&self.cfg.id).cloned()
        })
    }

    /// A command that does not need the connection.
    fn local(&mut self, cmd: TermCmd) {
        match cmd {
            TermCmd::Input { id, data } => {
                if let Some(pty) = self.slots.get(&id).and_then(|s| s.pty.as_ref()) {
                    let _ = pty.send(PtyIn::Data(data));
                }
            }
            TermCmd::Resize { id, size } => {
                if let Some(slot) = self.slots.get_mut(&id) {
                    slot.spec.size = size;
                    if let Some(pty) = &slot.pty {
                        let _ = pty.send(PtyIn::Resize(size));
                    }
                }
            }
            TermCmd::Close { id } => {
                if let Some(pty) = self.slots.remove(&id).and_then(|s| s.pty) {
                    let _ = pty.send(PtyIn::Close);
                }
            }
            TermCmd::Open { spec, reply } => self.pending.push_back((spec, reply)),
            TermCmd::Retry | TermCmd::Stop => {}
        }
    }

    /// Run `fut` (connecting or attaching) while handling commands that do
    /// not need the connection; `None` if told to stop. `Retry` is ignored:
    /// an attempt is under way.
    async fn busy<F: Future>(&mut self, fut: F) -> Option<F::Output> {
        tokio::pin!(fut);
        loop {
            tokio::select! {
                out = &mut fut => return Some(out),
                cmd = self.cmds.recv() => match cmd {
                    None | Some(TermCmd::Stop) => return None,
                    Some(cmd) => self.local(cmd),
                },
            }
        }
    }

    /// Attach `spec` over the current connection and start forwarding its
    /// output; `None` if told to stop meanwhile.
    async fn attach(
        &mut self,
        id: u64,
        generation: u64,
        spec: &AttachSpec,
    ) -> Option<Result<UnboundedSender<PtyIn>, OpenError>> {
        let zellij = self.zellij_path();
        let mut t = match std::mem::replace(&mut self.link, Link::Busy) {
            Link::Up(t) => t,
            other => {
                self.link = other;
                return Some(Err(OpenError::Retry("terminal connection is down".into())));
            }
        };
        let spec = spec.clone();
        let (t, r) = self
            .busy(async move {
                let r = t.attach(zellij.as_deref(), &spec).await;
                (t, r)
            })
            .await?;
        self.link = Link::Up(t);
        let PtyIo { input, mut output } = match r {
            Ok(io) => io,
            Err(e) => return Some(Err(e)),
        };
        let tx = self.out_tx.clone();
        tokio::spawn(async move {
            while let Some(o) = output.recv().await {
                if tx.send((id, generation, Some(o))).is_err() {
                    return;
                }
            }
            let _ = tx.send((id, generation, None));
        });
        Some(Ok(input))
    }

    /// Open the connection; `None` if told to stop meanwhile.
    async fn connect(&mut self) -> Option<Result<(), OpenError>> {
        let (connector, cfg) = (self.connector.clone(), self.cfg.clone());
        let r = self
            .busy(async move { connector.open_terminals(&cfg).await })
            .await?;
        Some(r.map(|t| {
            self.link = Link::Up(t);
            self.up_since = Some(Instant::now());
            self.emit(Some(ConnState::Up));
        }))
    }

    /// Drop the connection when the last terminal is gone.
    fn idle_if_empty(&mut self) {
        if self.slots.is_empty() && !matches!(self.link, Link::Idle | Link::Busy) {
            self.link = Link::Idle;
            self.up_since = None;
            self.backoff.reset();
            self.emit(None);
        }
    }

    /// `None` if told to stop meanwhile.
    async fn open(&mut self, spec: AttachSpec) -> Option<OpenResult> {
        match self.link {
            Link::Down { .. } => {
                // Detached terminals exist: reconnect and reattach them all,
                // or they would stay detached once the link is up again.
                if !self.reconnect().await {
                    return None;
                }
                if let Link::Down { error, .. } = &self.link {
                    return Some(Err(TermError::Open(error.clone())));
                }
            }
            Link::Idle => match self.connect().await? {
                Ok(()) => {}
                Err(e) => {
                    self.link = Link::Idle;
                    self.emit(None);
                    return Some(Err(TermError::Open(e)));
                }
            },
            Link::Up(_) | Link::Busy => {}
        }
        let id = self.next_id;
        self.next_id += 1;
        match self.attach(id, 0, &spec).await? {
            Ok(pty) => {
                let (tx, rx) = mpsc::unbounded_channel();
                let _ = tx.send(TermEvent::Attached);
                self.slots.insert(
                    id,
                    Slot {
                        spec,
                        events: tx,
                        pty: Some(pty),
                        generation: 0,
                        attached_at: Instant::now(),
                        quick_ends: 0,
                    },
                );
                Some(Ok((id, rx)))
            }
            Err(e) => {
                self.idle_if_empty();
                Some(Err(TermError::Open(e)))
            }
        }
    }

    /// The connection is gone: detach every terminal and schedule a
    /// reconnect. The backoff starts over only if the connection had
    /// stayed up for `STABLE_AFTER`.
    fn lost(&mut self, reason: String) {
        for slot in self.slots.values_mut() {
            slot.pty = None;
            let _ = slot.events.send(TermEvent::Detached {
                reason: reason.clone(),
            });
        }
        if self
            .up_since
            .take()
            .is_some_and(|t| t.elapsed() >= STABLE_AFTER)
        {
            self.backoff.reset();
        }
        let retry_in = self.backoff.next_delay();
        self.link = Link::Down {
            at: Some(Instant::now() + retry_in),
            error: OpenError::Retry(reason.clone()),
        };
        self.emit(Some(ConnState::Retrying { retry_in, reason }));
    }

    /// Reconnect and attach every terminal again; false if told to stop.
    async fn reconnect(&mut self) -> bool {
        match self.connect().await {
            None => return false,
            Some(Ok(())) => {}
            Some(Err(OpenError::Retry(reason))) => {
                let retry_in = self.backoff.next_delay();
                self.link = Link::Down {
                    at: Some(Instant::now() + retry_in),
                    error: OpenError::Retry(reason.clone()),
                };
                self.emit(Some(ConnState::Retrying { retry_in, reason }));
                return true;
            }
            Some(Err(OpenError::NeedsUser(p))) => {
                self.link = Link::Down {
                    at: None,
                    error: OpenError::NeedsUser(p.clone()),
                };
                self.emit(Some(ConnState::NeedsUser(p)));
                return true;
            }
        }
        let ids: Vec<u64> = self.slots.keys().copied().collect();
        for id in ids {
            let Some(slot) = self.slots.get(&id) else {
                continue;
            };
            let (spec, generation) = (slot.spec.clone(), slot.generation + 1);
            match self.attach(id, generation, &spec).await {
                None => return false,
                Some(Ok(pty)) => match self.slots.get_mut(&id) {
                    Some(slot) => {
                        slot.pty = Some(pty);
                        slot.generation = generation;
                        slot.attached_at = Instant::now();
                        let _ = slot.events.send(TermEvent::Attached);
                    }
                    // Closed while attaching.
                    None => {
                        let _ = pty.send(PtyIn::Close);
                    }
                },
                Some(Err(e)) => {
                    self.lost(reason_of(e));
                    return true;
                }
            }
        }
        self.idle_if_empty();
        true
    }

    /// One terminal's channel closed without an exit status while the
    /// connection is up: attach that terminal again, unless its channel
    /// keeps closing right after attaching. False if told to stop.
    async fn channel_ended(&mut self, id: u64) -> bool {
        let Some(slot) = self.slots.get_mut(&id) else {
            return true;
        };
        slot.pty = None;
        slot.quick_ends = if slot.attached_at.elapsed() < STABLE_AFTER {
            slot.quick_ends + 1
        } else {
            0
        };
        if slot.quick_ends >= MAX_QUICK_ENDS {
            let _ = slot.events.send(TermEvent::Exited(None));
            self.slots.remove(&id);
            self.idle_if_empty();
            return true;
        }
        let _ = slot.events.send(TermEvent::Detached {
            reason: "terminal channel closed".into(),
        });
        let (spec, generation) = (slot.spec.clone(), slot.generation + 1);
        match self.attach(id, generation, &spec).await {
            None => false,
            Some(Ok(pty)) => {
                match self.slots.get_mut(&id) {
                    Some(slot) => {
                        slot.pty = Some(pty);
                        slot.generation = generation;
                        slot.attached_at = Instant::now();
                        let _ = slot.events.send(TermEvent::Attached);
                    }
                    None => {
                        let _ = pty.send(PtyIn::Close);
                    }
                }
                self.idle_if_empty();
                true
            }
            Some(Err(e)) => {
                self.lost(reason_of(e));
                true
            }
        }
    }

    /// False if told to stop.
    async fn output(&mut self, (id, generation, out): Tagged) -> bool {
        let Some(slot) = self.slots.get(&id) else {
            return true;
        };
        if slot.generation != generation || slot.pty.is_none() {
            return true;
        }
        match out {
            Some(PtyOut::Data(d)) => {
                let _ = slot.events.send(TermEvent::Output(d));
            }
            Some(PtyOut::Exit(status)) => {
                let _ = slot.events.send(TermEvent::Exited(status));
                self.slots.remove(&id);
                self.idle_if_empty();
            }
            None => {
                if matches!(&self.link, Link::Up(t) if t.alive()) {
                    return self.channel_ended(id).await;
                }
                self.lost("connection lost".into());
            }
        }
        true
    }

    /// Handle one command; false when the task should end.
    async fn command(&mut self, cmd: Option<TermCmd>) -> bool {
        match cmd {
            None | Some(TermCmd::Stop) => false,
            Some(TermCmd::Open { spec, reply }) => match self.open(spec).await {
                Some(r) => {
                    let _ = reply.send(r);
                    true
                }
                None => false,
            },
            Some(TermCmd::Retry) => {
                if matches!(self.link, Link::Down { .. }) {
                    return self.reconnect().await;
                }
                true
            }
            Some(cmd) => {
                self.local(cmd);
                self.idle_if_empty();
                true
            }
        }
    }

    /// Close every PTY (the task is ending).
    fn close_all(&self) {
        for slot in self.slots.values() {
            if let Some(pty) = &slot.pty {
                let _ = pty.send(PtyIn::Close);
            }
        }
    }
}

/// Run the terminals of `cfg` until `TermCmd::Stop` or until the command
/// channel closes. Connection states go to `events` as `HostEvent::Term`.
pub async fn run_terms<C: Connector>(
    cfg: HostConfig,
    connector: Arc<C>,
    zellij: ZellijPaths,
    events: UnboundedSender<(String, HostEvent)>,
    cmds: UnboundedReceiver<TermCmd>,
) {
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let mut t = Terms {
        cfg,
        connector,
        zellij,
        events,
        cmds,
        link: Link::Idle,
        slots: HashMap::new(),
        pending: VecDeque::new(),
        next_id: 1,
        backoff: Backoff::default(),
        up_since: None,
        out_tx,
    };
    loop {
        let go_on = if let Some((spec, reply)) = t.pending.pop_front() {
            match t.open(spec).await {
                Some(r) => {
                    let _ = reply.send(r);
                    true
                }
                None => false,
            }
        } else {
            let retry_at = match t.link {
                Link::Down { at: Some(at), .. } if !t.slots.is_empty() => Some(at),
                _ => None,
            };
            let sleep = tokio::time::sleep_until(retry_at.unwrap_or_else(Instant::now));
            tokio::select! {
                cmd = t.cmds.recv() => t.command(cmd).await,
                Some(out) = out_rx.recv() => t.output(out).await,
                _ = sleep, if retry_at.is_some() => t.reconnect().await,
            }
        };
        if !go_on {
            t.close_all();
            return;
        }
    }
}
