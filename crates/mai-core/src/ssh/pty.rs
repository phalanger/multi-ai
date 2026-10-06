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

/// The server's answer to the last request: `true` accepted, `false`
/// refused. Output that arrives first is kept in `early`. No answer in
/// time is an error: a late answer would otherwise be taken for the answer
/// to the next request.
async fn reply(
    read: &mut ChannelReadHalf,
    early: &mut Vec<u8>,
    what: &str,
) -> Result<bool, SshError> {
    let wait = async {
        loop {
            match read.wait().await {
                Some(ChannelMsg::Success) => return Ok(true),
                Some(ChannelMsg::Failure) => return Ok(false),
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    early.extend_from_slice(&data);
                }
                Some(ChannelMsg::Close) | None => {
                    return Err(SshError::Channel("channel closed".into()));
                }
                Some(_) => {}
            }
        }
    };
    match tokio::time::timeout(REPLY_TIMEOUT, wait).await {
        Ok(r) => r,
        Err(_) => Err(SshError::Channel(format!(
            "no answer to the {what} request within {}s",
            REPLY_TIMEOUT.as_secs()
        ))),
    }
}

/// Request the PTY, the variables and the command; output seen before the
/// command was accepted is returned.
async fn setup(
    read: &mut ChannelReadHalf,
    write: &Writer,
    size: TermSize,
    env: &[(&str, &str)],
    command: impl FnOnce(bool) -> String,
) -> Result<Vec<u8>, SshError> {
    let mut early = Vec::new();
    let (cols, rows) = (u32::from(size.cols), u32::from(size.rows));
    write
        .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
        .await
        .map_err(chan_err)?;
    if !reply(read, &mut early, "pty").await? {
        return Err(SshError::Channel("server refused the pty request".into()));
    }
    let mut accepted = true;
    for (k, v) in env {
        write.set_env(true, *k, *v).await.map_err(chan_err)?;
        accepted &= reply(read, &mut early, "env").await?;
    }
    write
        .exec(true, command(accepted))
        .await
        .map_err(chan_err)?;
    if !reply(read, &mut early, "exec").await? {
        return Err(SshError::Channel("server refused the command".into()));
    }
    Ok(early)
}

/// Request a PTY of `size` and the variables in `env` on `ch`, then run
/// `command(env_accepted)`, where `env_accepted` says whether the server
/// accepted every variable (so the caller can fall back to a command
/// prefix). The returned `PtyIo` is driven by a spawned task. If setting
/// up fails, the channel is closed.
pub(crate) async fn start(
    ch: Channel<client::Msg>,
    size: TermSize,
    env: &[(&str, &str)],
    command: impl FnOnce(bool) -> String,
) -> Result<PtyIo, SshError> {
    let (mut read, write) = ch.split();
    let early = match setup(&mut read, &write, size, env, command).await {
        Ok(early) => early,
        Err(e) => {
            let _ = write.close().await;
            return Err(e);
        }
    };
    let (io, ends) = pty_channels();
    if !early.is_empty() {
        let _ = ends.output.send(PtyOut::Data(early));
    }
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
