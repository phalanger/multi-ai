//! The last part of a probe's stderr, kept so that a probe that stops
//! (bad rules file, pid file error, ...) can say why instead of only
//! "probe stream closed".

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};

/// Bytes of stderr kept.
pub const TAIL_BYTES: usize = 2048;

/// How long to wait for stderr to end after stdout did (the two pipes
/// close independently).
pub const SETTLE: Duration = Duration::from_millis(300);

#[derive(Debug, Default)]
pub struct StderrTail {
    text: Mutex<String>,
    done: AtomicBool,
}

impl StderrTail {
    /// Append `bytes`, keeping only the last `TAIL_BYTES` bytes (cut at a
    /// character boundary).
    pub fn push(&self, bytes: &[u8]) {
        let mut text = self.text.lock().unwrap_or_else(|p| p.into_inner());
        text.push_str(&String::from_utf8_lossy(bytes));
        if text.len() > TAIL_BYTES {
            let mut cut = text.len() - TAIL_BYTES;
            while !text.is_char_boundary(cut) {
                cut += 1;
            }
            text.drain(..cut);
        }
    }

    /// Stderr has ended.
    pub fn finish(&self) {
        self.done.store(true, Ordering::SeqCst);
    }

    /// The last (at most three) non-empty lines, joined with ` | `, after
    /// waiting up to `SETTLE` for stderr to end; `None` if there are none.
    pub async fn summary(&self) -> Option<String> {
        let deadline = tokio::time::Instant::now() + SETTLE;
        while !self.done.load(Ordering::SeqCst) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let text = self.text.lock().unwrap_or_else(|p| p.into_inner());
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let last = &lines[lines.len().saturating_sub(3)..];
        (!last.is_empty()).then(|| last.join(" | "))
    }
}

/// Copy `reader` into `tail` until it ends.
pub async fn collect(mut reader: impl AsyncRead + Unpin, tail: std::sync::Arc<StderrTail>) {
    let mut buf = [0u8; 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => tail.push(&buf[..n]),
        }
    }
    tail.finish();
}
