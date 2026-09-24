//! `serve` runtime: the JSON Lines loop around `Server`. Lives until the
//! app side closes stdin (SSH channel gone) or stdout breaks.

use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mai_protocol::{ProbeMsg, encode_line};

use crate::metrics::MetricsSampler;
use crate::serve::{Server, due};
use crate::zellij::Zellij;

const TICK: Duration = Duration::from_millis(250);
const HEARTBEAT_MS: u64 = 5_000;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn send(out: &mut impl Write, msgs: &[ProbeMsg]) -> io::Result<()> {
    for m in msgs {
        out.write_all(encode_line(m)?.as_bytes())?;
    }
    out.flush()
}

/// Send `hello`, then loop: apply input lines, tick the server, and emit
/// metrics and heartbeats on their intervals. Returns when `input` ends.
pub fn run<Z: Zellij>(
    mut server: Server<Z>,
    hello: ProbeMsg,
    input: impl BufRead + Send + 'static,
    mut out: impl Write,
) -> io::Result<()> {
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut input = input;
        let mut buf = Vec::new();
        loop {
            buf.clear();
            // Bytes, not `lines()`: a non-UTF-8 line must be reported as a
            // bad message, not end the loop.
            match input.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let line = String::from_utf8_lossy(&buf);
            let line = line.trim_end_matches(['\r', '\n']).to_owned();
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    send(&mut out, &[hello])?;
    let mut metrics = MetricsSampler::new();
    let (mut last_metrics, mut last_beat) = (None, None);
    loop {
        let mut msgs = Vec::new();
        match rx.recv_timeout(TICK) {
            Ok(line) => msgs.extend(server.handle_line(&line)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }
        while let Ok(line) = rx.try_recv() {
            msgs.extend(server.handle_line(&line));
        }
        let now = now_ms();
        msgs.extend(server.tick(now));
        if due(last_metrics, server.intervals().metrics_ms, now) {
            last_metrics = Some(now);
            msgs.push(ProbeMsg::Metrics(metrics.sample(now)));
        }
        if due(last_beat, HEARTBEAT_MS, now) {
            last_beat = Some(now);
            msgs.push(ProbeMsg::Heartbeat { ts_ms: now });
        }
        send(&mut out, &msgs)?;
    }
}
