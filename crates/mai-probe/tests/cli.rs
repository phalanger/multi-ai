//! Contract of the `mai-probe` binary as agents and the app call it.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

/// Run the probe with `MAI_HOME=home`, feeding `stdin`. `in_pane` sets
/// the zellij variables of session `work`, pane 5; otherwise they are
/// removed.
fn probe(home: &Path, args: &[&str], stdin: &str, in_pane: bool) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mai-probe"));
    cmd.args(args)
        .env("MAI_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if in_pane {
        cmd.env("ZELLIJ_SESSION_NAME", "work")
            .env("ZELLIJ_PANE_ID", "5");
    } else {
        cmd.env_remove("ZELLIJ_SESSION_NAME")
            .env_remove("ZELLIJ_PANE_ID");
    }
    let mut child = cmd.spawn().unwrap();
    // The probe may exit before reading stdin (usage errors): ignore EPIPE.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    child.wait_with_output().unwrap()
}

/// All spool lines under `<home>/spool/*.jsonl`.
fn spool_lines(home: &Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(home.join("spool")) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for e in entries {
        let path = e.unwrap().path();
        if path.extension().is_some_and(|x| x == "jsonl") {
            let text = std::fs::read_to_string(path).unwrap();
            lines.extend(text.lines().map(|l| serde_json::from_str(l).unwrap()));
        }
    }
    lines
}

const STOP: &str = r#"{"hook_event_name": "Stop", "last_assistant_message": "ok"}"#;

#[test]
fn hook_appends_one_spool_line_silently() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(home.path(), &["hook", "claude"], STOP, true);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    let lines = spool_lines(home.path());
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["pane_id"], 5);
    assert_eq!(lines[0]["session"], "work");
    assert_eq!(lines[0]["agent"], "claude");
}

#[test]
fn hook_with_invalid_json_exits_zero_without_spool() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(home.path(), &["hook", "claude"], "{ nope", true);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(spool_lines(home.path()).is_empty());
}

#[test]
fn hook_outside_zellij_exits_zero_without_spool() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(home.path(), &["hook", "claude"], STOP, false);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(spool_lines(home.path()).is_empty());
}

#[test]
fn hook_usage_error_never_blocks_the_agent() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(home.path(), &["hook", "claude", "--nope"], STOP, true);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn emit_rejects_unknown_state() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(
        home.path(),
        &["emit", "--agent", "a", "--state", "bogus"],
        "",
        true,
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(spool_lines(home.path()).is_empty());
}

#[test]
fn emit_outside_zellij_fails() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(
        home.path(),
        &["emit", "--agent", "a", "--state", "done"],
        "",
        false,
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(spool_lines(home.path()).is_empty());
}
