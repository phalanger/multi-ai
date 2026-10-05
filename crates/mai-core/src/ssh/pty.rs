//! A PTY on an SSH session channel, bridged to `PtyIo`.

use std::time::Duration;

use russh::client;
use russh::{Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf};

use super::client::SshError;
use crate::pty::{PtyEnds, PtyIn, PtyIo, PtyOut, TermSize, pty_channels};

/// How long to wait for the server to answer a channel request.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

type Writer = ChannelWriteHalf<client::Msg>;

fn chan_err(e: impl std::fmt::Display) -> SshError {
    SshError::Channel(e.to_string())
}

/// The server's answer to the last request: `Some(true)` accepted,
/// `Some(false)` refused, `None` no answer in time.
async fn reply(read: &mut ChannelReadHalf) -> Result<Option<bool>, SshError> {
    let wait = async {
        loop {
            match read.wait().await {
                Some(ChannelMsg::Success) => return Ok(true),
                Some(ChannelMsg::Failure) => return Ok(false),
                Some(ChannelMsg::Close) | None => {
                    return Err(SshError::Channel("channel closed".into()));
                }
                Some(_) => {}
            }
        }
    };
    match tokio::time::timeout(REPLY_TIMEOUT, wait).await {
        Ok(r) => r.map(Some),
        Err(_) => Ok(None),
    }
}

/// Request a PTY of `size` and the variables in `env` on `ch`, then run
/// `command(env_accepted)`, where `env_accepted` says whether the server
/// accepted every variable (so the caller can fall back to a command
/// prefix). The returned `PtyIo` is driven by a spawned task.
pub(crate) async fn start(
    ch: Channel<client::Msg>,
    size: TermSize,
    env: &[(&str, &str)],
    command: impl FnOnce(bool) -> String,
) -> Result<PtyIo, SshError> {
    let (mut read, write) = ch.split();
    let (cols, rows) = (u32::from(size.cols), u32::from(size.rows));
    write
        .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
        .await
        .map_err(chan_err)?;
    if reply(&mut read).await? != Some(true) {
        return Err(SshError::Channel("server refused the pty request".into()));
    }
    let mut accepted = true;
    for (k, v) in env {
        write.set_env(true, *k, *v).await.map_err(chan_err)?;
        accepted &= reply(&mut read).await? == Some(true);
    }
    write
        .exec(true, command(accepted))
        .await
        .map_err(chan_err)?;
    if reply(&mut read).await? == Some(false) {
        return Err(SshError::Channel("server refused the command".into()));
    }
    let (io, ends) = pty_channels();
    tokio::spawn(pump(read, write, ends));
    Ok(io)
}

/// Move bytes both ways until the server closes the channel. `Exit` is
/// sent only when the program's exit was reported; a channel that ends
/// without it (connection lost) just ends the output.
async fn pump(mut read: ChannelReadHalf, write: Writer, ends: PtyEnds) {
    let PtyEnds { mut input, output } = ends;
    let mut status = None;
    let mut exited = false;
    let mut input_open = true;
    loop {
        tokio::select! {
            msg = read.wait() => match msg {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    if output.send(PtyOut::Data(data.to_vec())).is_err() {
                        let _ = write.close().await;
                        return;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    status = Some(exit_status);
                    exited = true;
                }
                Some(ChannelMsg::ExitSignal { .. }) => exited = true,
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            cmd = input.recv(), if input_open => match cmd {
                Some(PtyIn::Data(d)) => {
                    if write.data(&d[..]).await.is_err() {
                        break;
                    }
                }
                Some(PtyIn::Resize(s)) => {
                    let _ = write
                        .window_change(u32::from(s.cols), u32::from(s.rows), 0, 0)
                        .await;
                }
                Some(PtyIn::Close) | None => {
                    input_open = false;
                    let _ = write.eof().await;
                    let _ = write.close().await;
                }
            },
        }
    }
    if exited {
        let _ = output.send(PtyOut::Exit(status));
    }
}
