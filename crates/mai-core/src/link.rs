//! The app side of the probe's JSON Lines stream: handshake, receiving
//! `ProbeMsg`s with a silence watchdog, and sending `AppMsg`s. Knows
//! nothing about SSH or processes; any byte stream pair works.

use std::fmt;
use std::time::Duration;

use mai_protocol::{AppMsg, PROTOCOL_VERSION, ProbeMsg, decode_line, encode_line};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, Lines};

/// The probe must say `Hello` within this time after `serve` starts
/// (finding zellij may run a login shell).
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(30);

/// The probe sends a heartbeat every 5 s; this much silence means the
/// stream is dead even if the transport has not noticed.
pub const SILENCE_TIMEOUT: Duration = Duration::from_secs(20);

/// Lines that are not protocol messages (e.g. printed by a shell rc file)
/// tolerated before `Hello`.
pub const MAX_NOISE_LINES: usize = 50;

pub type LineReader = Box<dyn AsyncBufRead + Send + Unpin>;
pub type LineWriter = Box<dyn AsyncWrite + Send + Unpin>;

/// A started `serve`: its stdout to read and stdin to write.
pub struct ProbeIo {
    pub reader: LineReader,
    pub writer: LineWriter,
}

/// What the probe reported about itself in `Hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloInfo {
    pub protocol_version: u32,
    pub probe_version: String,
    pub os: String,
    pub arch: String,
    pub zellij_path: Option<String>,
    pub zellij_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// Reading or writing the stream failed.
    Io(String),
    /// The probe closed its output (process exited or channel closed).
    Closed,
    /// No `Hello` in time, or nothing at all for `SILENCE_TIMEOUT`.
    Silent,
    /// Output before `Hello` was not a protocol stream.
    NoHello(String),
    /// The probe speaks another protocol version.
    Protocol { probe: u32, app: u32 },
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "probe stream: {e}"),
            Self::Closed => f.write_str("probe stream closed"),
            Self::Silent => f.write_str("probe stopped responding"),
            Self::NoHello(m) => write!(f, "probe did not start: {m}"),
            Self::Protocol { probe, app } => {
                write!(f, "probe protocol {probe}, app protocol {app}")
            }
        }
    }
}

impl std::error::Error for LinkError {}

pub struct ProbeLink {
    lines: Lines<LineReader>,
    writer: LineWriter,
}

impl ProbeLink {
    pub fn new(io: ProbeIo) -> Self {
        Self {
            lines: io.reader.lines(),
            writer: io.writer,
        }
    }

    async fn next_line(&mut self, timeout: Duration) -> Result<String, LinkError> {
        match tokio::time::timeout(timeout, self.lines.next_line()).await {
            Err(_) => Err(LinkError::Silent),
            Ok(Err(e)) => Err(LinkError::Io(e.to_string())),
            Ok(Ok(None)) => Err(LinkError::Closed),
            Ok(Ok(Some(line))) => Ok(line),
        }
    }

    /// Wait for `Hello`, skipping up to `MAX_NOISE_LINES` lines that are
    /// not protocol messages, and check the protocol version.
    pub async fn hello(&mut self) -> Result<HelloInfo, LinkError> {
        let deadline = tokio::time::Instant::now() + HELLO_TIMEOUT;
        let mut noise = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let line = self.next_line(left).await?;
            match decode_line::<ProbeMsg>(&line) {
                Ok(ProbeMsg::Hello {
                    protocol_version,
                    probe_version,
                    os,
                    arch,
                    zellij_path,
                    zellij_version,
                }) => {
                    if protocol_version != PROTOCOL_VERSION {
                        return Err(LinkError::Protocol {
                            probe: protocol_version,
                            app: PROTOCOL_VERSION,
                        });
                    }
                    return Ok(HelloInfo {
                        protocol_version,
                        probe_version,
                        os,
                        arch,
                        zellij_path,
                        zellij_version,
                    });
                }
                Ok(other) => {
                    return Err(LinkError::NoHello(format!(
                        "first message was not hello: {other:?}"
                    )));
                }
                Err(_) => {
                    noise.push(line);
                    if noise.len() > MAX_NOISE_LINES {
                        let head: String = noise[0].chars().take(120).collect();
                        return Err(LinkError::NoHello(format!("unexpected output: {head}")));
                    }
                }
            }
        }
    }

    /// Next message. A line that does not parse is returned as
    /// `ProbeMsg::Error` with code `bad_probe_line`, not as a failure.
    pub async fn recv(&mut self) -> Result<ProbeMsg, LinkError> {
        let line = self.next_line(SILENCE_TIMEOUT).await?;
        Ok(decode_line(&line).unwrap_or_else(|e| {
            let head: String = line.chars().take(120).collect();
            ProbeMsg::Error {
                code: "bad_probe_line".to_owned(),
                message: format!("{e}: {head}"),
            }
        }))
    }

    pub async fn send(&mut self, msg: &AppMsg) -> Result<(), LinkError> {
        let line = encode_line(msg).map_err(|e| LinkError::Io(e.to_string()))?;
        self.writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| LinkError::Io(e.to_string()))?;
        self.writer
            .flush()
            .await
            .map_err(|e| LinkError::Io(e.to_string()))
    }
}
