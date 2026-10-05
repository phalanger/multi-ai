//! A terminal byte stream: a program (normally `zellij attach`) running
//! in a PTY, wherever that PTY lives. Every backend is bridged to the
//! same pair of channels, so terminal logic and its tests see one shape.
//!
//! Output that ends with `PtyOut::Exit` means the program exited. Output
//! that ends without it means the connection carrying the PTY was lost.

use std::ffi::OsString;
use std::io::{self, Read, Write};

use std::thread;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

/// Terminal size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

/// Sent to the PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyIn {
    Data(Vec<u8>),
    Resize(TermSize),
    /// End the program (for `zellij attach`: detach this client).
    Close,
}

/// Received from the PTY.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PtyOut {
    Data(Vec<u8>),
    /// The program exited, with its status when known.
    Exit(Option<u32>),
}

/// The app's ends of a running PTY.
pub struct PtyIo {
    pub input: UnboundedSender<PtyIn>,
    pub output: UnboundedReceiver<PtyOut>,
}

/// The backend's ends of a PTY.
pub struct PtyEnds {
    pub input: UnboundedReceiver<PtyIn>,
    pub output: UnboundedSender<PtyOut>,
}

/// A connected pair: what the app holds and what a backend drives.
pub fn pty_channels() -> (PtyIo, PtyEnds) {
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (out_tx, out_rx) = mpsc::unbounded_channel();
    (
        PtyIo {
            input: in_tx,
            output: out_rx,
        },
        PtyEnds {
            input: in_rx,
            output: out_tx,
        },
    )
}

fn pty_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

fn pty_size(size: TermSize) -> PtySize {
    PtySize {
        rows: size.rows,
        cols: size.cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// Run `program args` in a local PTY (ConPTY on Windows) with extra
/// environment variables. Three threads bridge the blocking PTY API:
/// one reads output, one waits for the program, one applies input.
pub fn spawn_local(
    program: &OsString,
    args: &[String],
    env: &[(&str, &str)],
    size: TermSize,
) -> io::Result<PtyIo> {
    let pair = native_pty_system()
        .openpty(pty_size(size))
        .map_err(pty_err)?;
    let mut cmd = CommandBuilder::new(program);
    cmd.args(args);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = pair.slave.spawn_command(cmd).map_err(pty_err)?;
    // Only the child keeps the slave open, so its exit ends the output.
    drop(pair.slave);
    let mut reader = pair.master.try_clone_reader().map_err(pty_err)?;
    let mut writer = pair.master.take_writer().map_err(pty_err)?;
    let mut killer = child.clone_killer();
    let master = pair.master;
    let (io, ends) = pty_channels();
    let PtyEnds {
        input: mut in_rx,
        output: out_tx,
    } = ends;

    // Input thread: owns the master, so dropping it (on close or after the
    // program exits) closes the PTY; on Windows that is what ends output.
    let input_thread = thread::spawn(move || {
        while let Some(msg) = in_rx.blocking_recv() {
            match msg {
                PtyIn::Data(d) => {
                    if writer.write_all(&d).and_then(|()| writer.flush()).is_err() {
                        break;
                    }
                }
                PtyIn::Resize(s) => {
                    let _ = master.resize(pty_size(s));
                }
                PtyIn::Close => {
                    let _ = killer.kill();
                    break;
                }
            }
        }
        drop(writer);
        drop(master);
    });

    // Waiter thread: the exit status, then wake the input thread so it
    // drops the master (killing an exited program is harmless).
    let wake = io.input.clone();
    let waiter = thread::spawn(move || {
        let status = child.wait().ok().map(|s| s.exit_code());
        let _ = wake.send(PtyIn::Close);
        status
    });

    // Reader thread: all output, then the exit status once output ends.
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if out_tx.send(PtyOut::Data(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
        let status = waiter.join().ok().flatten();
        let _ = input_thread.join();
        let _ = out_tx.send(PtyOut::Exit(status));
    });
    Ok(io)
}
