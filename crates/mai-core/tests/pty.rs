//! Local PTY backend against real programs (cmd on Windows, sh elsewhere).

use std::ffi::OsString;
use std::time::Duration;

use mai_core::pty::{PtyIn, PtyIo, PtyOut, TermSize, spawn_local};
use tokio::time::timeout;

const SIZE: TermSize = TermSize { cols: 80, rows: 24 };

/// `script` run by the platform shell in a PTY.
fn shell(script: &str) -> PtyIo {
    let (program, flag) = if cfg!(windows) {
        ("cmd", "/c")
    } else {
        ("sh", "-c")
    };
    let args = vec![flag.to_owned(), script.to_owned()];
    spawn_local(&OsString::from(program), &args, &[], SIZE).expect("spawn in pty")
}

/// Cursor position request. ConPTY sends it at startup and waits for the
/// terminal's answer; in the app xterm.js answers, here the test does.
const DSR: &[u8] = b"\x1b[6n";

/// Everything the program prints, and its exit status.
async fn finish(mut io: PtyIo) -> (String, Option<u32>) {
    let mut text = Vec::new();
    loop {
        match timeout(Duration::from_secs(20), io.output.recv()).await {
            Err(_) => panic!(
                "pty did not finish; output so far: {:?}",
                String::from_utf8_lossy(&text)
            ),
            Ok(None) => panic!("output ended without Exit"),
            Ok(Some(PtyOut::Data(d))) => {
                if d.windows(DSR.len()).any(|w| w == DSR) {
                    let _ = io.input.send(PtyIn::Data(b"\x1b[1;1R".to_vec()));
                }
                text.extend_from_slice(&d);
            }
            Ok(Some(PtyOut::Exit(status))) => {
                return (String::from_utf8_lossy(&text).into_owned(), status);
            }
        }
    }
}

#[tokio::test]
async fn output_and_exit_status_arrive() {
    let (text, status) = finish(shell("echo hello-pty")).await;
    assert!(text.contains("hello-pty"), "{text:?}");
    assert_eq!(status, Some(0));
}

#[tokio::test]
async fn nonzero_exit_status_is_reported() {
    let (_, status) = finish(shell("exit 3")).await;
    assert_eq!(status, Some(3));
}

#[tokio::test]
async fn input_reaches_the_program() {
    let script = if cfg!(windows) {
        "set /p X= & call echo got-%X%"
    } else {
        "read X; echo got-$X"
    };
    let io = shell(script);
    io.input.send(PtyIn::Data(b"abc\r".to_vec())).unwrap();
    let (text, _) = finish(io).await;
    assert!(text.contains("got-abc"), "{text:?}");
}

#[tokio::test]
async fn close_ends_a_running_program() {
    let script = if cfg!(windows) {
        "ping -n 30 127.0.0.1"
    } else {
        "sleep 30"
    };
    let io = shell(script);
    io.input
        .send(PtyIn::Resize(TermSize {
            cols: 100,
            rows: 30,
        }))
        .unwrap();
    io.input.send(PtyIn::Close).unwrap();
    let start = std::time::Instant::now();
    let (_, _) = finish(io).await;
    assert!(
        start.elapsed() < Duration::from_secs(15),
        "{:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn missing_program_is_an_error() {
    let r = spawn_local(&OsString::from("mai-no-such-program-xyz"), &[], &[], SIZE);
    assert!(r.is_err());
}
