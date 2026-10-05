//! Interactive terminals: `zellij attach <session>` in a PTY per open
//! terminal, over a host's terminal connection (TermConn, design 2.2).
//!
//! One task per host owns the terminal connection and all its PTYs. The
//! connection is opened with the first terminal and closed with the last.
//! When it is lost, every terminal is told it is detached, the task
//! reconnects with backoff and attaches each terminal again (zellij keeps
//! the session, so the user continues where they were).

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::deploy::{Remote, Shell};
use crate::host::{Backoff, ConnState, Connector, HostConfig, HostEvent, OpenError};
use crate::pty::{PtyIn, PtyIo, PtyOut, TermSize};

/// Locale requested for terminals (design 7): without it zellij sessions
/// created over SSH on macOS run in the C locale and CJK input breaks.
pub const LOCALE_ENV: [(&str, &str); 2] = [("LANG", "en_US.UTF-8"), ("LC_CTYPE", "en_US.UTF-8")];

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
}

/// Sent to the owner of a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    Output(Vec<u8>),
    /// Attached (again): zellij redraws the whole screen.
    Attached,
    /// The connection was lost; the terminal is reattached when it is back.
    Detached {
        reason: String,
    },
    /// `zellij attach` exited (the user detached or quit the session).
    /// The terminal is finished.
    Exited(Option<u32>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermError {
    /// No host with this id.
    NoHost,
    /// The terminal connection or `zellij attach` could not be started.
    Open(OpenError),
    /// The host was removed while opening.
    Stopped,
}

impl fmt::Display for TermError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHost => f.write_str("no such host"),
            Self::Open(OpenError::Retry(m)) => write!(f, "terminal connection failed: {m}"),
            Self::Open(OpenError::NeedsUser(p)) => write!(f, "{p}"),
            Self::Stopped => f.write_str("host removed"),
        }
    }
}

impl std::error::Error for TermError {}

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
}

/// Output of one PTY, tagged with its terminal and attach generation;
/// `None` means the output ended.
type Tagged = (u64, u64, Option<PtyOut>);

enum Link<T> {
    /// No terminals, no connection.
    Idle,
    Up(T),
    /// Lost; reconnect at the instant (or on `Retry` if `None`).
    Down(Option<Instant>),
}

struct Terms<C: Connector> {
    cfg: HostConfig,
    connector: Arc<C>,
    zellij: ZellijPaths,
    events: UnboundedSender<(String, HostEvent)>,
    link: Link<C::Terminals>,
    slots: HashMap<u64, Slot>,
    next_id: u64,
    backoff: Backoff,
    out_tx: UnboundedSender<Tagged>,
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

    /// Attach `spec` over the current connection and start forwarding its
    /// output.
    async fn attach(
        &mut self,
        id: u64,
        generation: u64,
        spec: &AttachSpec,
    ) -> Result<UnboundedSender<PtyIn>, OpenError> {
        let zellij = self.zellij_path();
        let Link::Up(t) = &mut self.link else {
            return Err(OpenError::Retry("terminal connection is down".into()));
        };
        let PtyIo { input, mut output } = t.attach(zellij.as_deref(), spec).await?;
        let tx = self.out_tx.clone();
        tokio::spawn(async move {
            while let Some(o) = output.recv().await {
                if tx.send((id, generation, Some(o))).is_err() {
                    return;
                }
            }
            let _ = tx.send((id, generation, None));
        });
        Ok(input)
    }

    async fn connect(&mut self) -> Result<(), OpenError> {
        let t = self.connector.open_terminals(&self.cfg).await?;
        self.link = Link::Up(t);
        self.emit(Some(ConnState::Up));
        Ok(())
    }

    /// Drop the connection when the last terminal is gone.
    fn idle_if_empty(&mut self) {
        if self.slots.is_empty() && !matches!(self.link, Link::Idle) {
            self.link = Link::Idle;
            self.backoff.reset();
            self.emit(None);
        }
    }

    async fn open(&mut self, spec: AttachSpec) -> OpenResult {
        if matches!(self.link, Link::Down(_)) {
            // Detached terminals exist: reconnect and reattach them all, or
            // they would stay detached once the link is up again.
            self.reconnect().await;
            if !matches!(self.link, Link::Up(_)) {
                return Err(TermError::Open(OpenError::Retry(
                    "terminal connection is down".into(),
                )));
            }
        } else if matches!(self.link, Link::Idle)
            && let Err(e) = self.connect().await
        {
            self.link = Link::Idle;
            self.emit(None);
            return Err(TermError::Open(e));
        }
        let id = self.next_id;
        self.next_id += 1;
        match self.attach(id, 0, &spec).await {
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
                    },
                );
                Ok((id, rx))
            }
            Err(e) => {
                self.idle_if_empty();
                Err(TermError::Open(e))
            }
        }
    }

    /// The connection is gone: detach every terminal and schedule a
    /// reconnect.
    fn lost(&mut self, reason: String) {
        for slot in self.slots.values_mut() {
            slot.pty = None;
            let _ = slot.events.send(TermEvent::Detached {
                reason: reason.clone(),
            });
        }
        let retry_in = self.backoff.next_delay();
        self.link = Link::Down(Some(Instant::now() + retry_in));
        self.emit(Some(ConnState::Retrying { retry_in, reason }));
    }

    /// Reconnect and attach every terminal again.
    async fn reconnect(&mut self) {
        if let Err(e) = self.connect().await {
            match e {
                OpenError::Retry(reason) => {
                    let retry_in = self.backoff.next_delay();
                    self.link = Link::Down(Some(Instant::now() + retry_in));
                    self.emit(Some(ConnState::Retrying { retry_in, reason }));
                }
                OpenError::NeedsUser(p) => {
                    self.link = Link::Down(None);
                    self.emit(Some(ConnState::NeedsUser(p)));
                }
            }
            return;
        }
        let ids: Vec<u64> = self.slots.keys().copied().collect();
        for id in ids {
            let Some(slot) = self.slots.get(&id) else {
                continue;
            };
            let (spec, generation) = (slot.spec.clone(), slot.generation + 1);
            match self.attach(id, generation, &spec).await {
                Ok(pty) => {
                    if let Some(slot) = self.slots.get_mut(&id) {
                        slot.pty = Some(pty);
                        slot.generation = generation;
                        let _ = slot.events.send(TermEvent::Attached);
                    }
                }
                Err(e) => {
                    let reason = match e {
                        OpenError::Retry(m) => m,
                        OpenError::NeedsUser(p) => p.to_string(),
                    };
                    return self.lost(reason);
                }
            }
        }
        self.backoff.reset();
    }

    fn output(&mut self, (id, generation, out): Tagged) {
        let Some(slot) = self.slots.get(&id) else {
            return;
        };
        if slot.generation != generation || slot.pty.is_none() {
            return;
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
            None => self.lost("connection lost".into()),
        }
    }

    /// Handle one command; false when the task should end.
    async fn command(&mut self, cmd: Option<TermCmd>) -> bool {
        match cmd {
            None | Some(TermCmd::Stop) => {
                for slot in self.slots.values() {
                    if let Some(pty) = &slot.pty {
                        let _ = pty.send(PtyIn::Close);
                    }
                }
                return false;
            }
            Some(TermCmd::Open { spec, reply }) => {
                let r = self.open(spec).await;
                let _ = reply.send(r);
            }
            Some(TermCmd::Input { id, data }) => {
                if let Some(pty) = self.slots.get(&id).and_then(|s| s.pty.as_ref()) {
                    let _ = pty.send(PtyIn::Data(data));
                }
            }
            Some(TermCmd::Resize { id, size }) => {
                if let Some(slot) = self.slots.get_mut(&id) {
                    slot.spec.size = size;
                    if let Some(pty) = &slot.pty {
                        let _ = pty.send(PtyIn::Resize(size));
                    }
                }
            }
            Some(TermCmd::Close { id }) => {
                if let Some(pty) = self.slots.remove(&id).and_then(|s| s.pty) {
                    let _ = pty.send(PtyIn::Close);
                }
                self.idle_if_empty();
            }
            Some(TermCmd::Retry) => {
                if matches!(self.link, Link::Down(_)) {
                    self.reconnect().await;
                }
            }
        }
        true
    }
}

/// Run the terminals of `cfg` until `TermCmd::Stop` or until the command
/// channel closes. Connection states go to `events` as `HostEvent::Term`.
pub async fn run_terms<C: Connector>(
    cfg: HostConfig,
    connector: Arc<C>,
    zellij: ZellijPaths,
    events: UnboundedSender<(String, HostEvent)>,
    mut cmds: UnboundedReceiver<TermCmd>,
) {
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let mut t = Terms {
        cfg,
        connector,
        zellij,
        events,
        link: Link::Idle,
        slots: HashMap::new(),
        next_id: 1,
        backoff: Backoff::default(),
        out_tx,
    };
    loop {
        let retry_at = match t.link {
            Link::Down(Some(at)) if !t.slots.is_empty() => Some(at),
            _ => None,
        };
        let sleep = tokio::time::sleep_until(retry_at.unwrap_or_else(Instant::now));
        tokio::select! {
            cmd = cmds.recv() => {
                if !t.command(cmd).await {
                    return;
                }
            }
            Some(out) = out_rx.recv() => t.output(out),
            _ = sleep, if retry_at.is_some() => t.reconnect().await,
        }
    }
}
