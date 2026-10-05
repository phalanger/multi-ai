use std::sync::Arc;

use mai_core::stderr::{StderrTail, TAIL_BYTES, collect};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn summary_is_the_last_three_non_empty_lines() {
    let t = StderrTail::default();
    t.push(b"one\n\ntwo\r\nthree\n");
    t.push(b"  four  \n\n");
    t.finish();
    assert_eq!(t.summary().await.as_deref(), Some("two | three | four"));
}

#[tokio::test]
async fn empty_stderr_has_no_summary() {
    let t = StderrTail::default();
    t.push(b"\n  \n");
    t.finish();
    assert_eq!(t.summary().await, None);
}

#[tokio::test]
async fn only_the_tail_is_kept() {
    let t = StderrTail::default();
    t.push(&vec![b'a'; TAIL_BYTES * 3]);
    t.push("\u{4e2d}end".as_bytes());
    t.finish();
    let s = t.summary().await.unwrap();
    assert!(s.ends_with("\u{4e2d}end"), "{s}");
    assert!(s.len() <= TAIL_BYTES, "{}", s.len());
}

#[tokio::test(start_paused = true)]
async fn summary_waits_briefly_for_stderr_to_end() {
    let t = Arc::new(StderrTail::default());
    let (mut w, r) = tokio::io::duplex(64);
    tokio::spawn(collect(r, t.clone()));
    w.write_all(b"error: bad rules file\n").await.unwrap();
    drop(w);
    assert_eq!(t.summary().await.as_deref(), Some("error: bad rules file"));
}
