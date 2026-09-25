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
fn serve_with_missing_explicit_zellij_reports_null_path() {
    let home = tempfile::tempdir().unwrap();
    let missing = home.path().join("no-zellij");
    let out = probe(
        home.path(),
        &["serve", "--zellij", missing.to_str().unwrap()],
        "",
        false,
    );
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    let hello: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(hello["type"], "hello", "{text}");
    assert_eq!(hello["zellij_path"], Value::Null, "{text}");
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

/// Start `serve` for `client` with stdin held open, so it keeps running.
fn spawn_serve(home: &Path, client: &str) -> std::process::Child {
    let missing = home.join("no-zellij");
    Command::new(env!("CARGO_BIN_EXE_mai-probe"))
        .args(["serve", "--client", client, "--zellij"])
        .arg(&missing)
        .env("MAI_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// Poll `f` every 50 ms for up to 10 s.
fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if f() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

fn pid_in(file: &Path) -> Option<u32> {
    std::fs::read_to_string(file).ok()?.trim().parse().ok()
}

#[test]
fn new_serve_replaces_old_one_and_stop_ends_it() {
    let home = tempfile::tempdir().unwrap();
    let pids = home.path().join("serve-t1.pid");
    let mut first = spawn_serve(home.path(), "t1");
    assert!(wait_for(|| pid_in(&pids) == Some(first.id())));

    let mut second = spawn_serve(home.path(), "t1");
    assert!(
        wait_for(|| first.try_wait().unwrap().is_some()),
        "old serve must be stopped"
    );
    assert!(wait_for(|| pid_in(&pids) == Some(second.id())));

    let out = probe(home.path(), &["stop", "--client", "t1"], "", false);
    assert_eq!(out.status.code(), Some(0));
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reply["stopped"], second.id());
    assert!(wait_for(|| second.try_wait().unwrap().is_some()));
}

#[test]
fn serves_of_different_clients_coexist() {
    let home = tempfile::tempdir().unwrap();
    let mut a = spawn_serve(home.path(), "a");
    assert!(wait_for(
        || pid_in(&home.path().join("serve-a.pid")) == Some(a.id())
    ));
    let mut b = spawn_serve(home.path(), "b");
    assert!(wait_for(
        || pid_in(&home.path().join("serve-b.pid")) == Some(b.id())
    ));
    assert!(
        a.try_wait().unwrap().is_none(),
        "other client's serve keeps running"
    );
    drop(a.stdin.take());
    drop(b.stdin.take());
    assert!(wait_for(|| a.try_wait().unwrap().is_some()));
    assert!(wait_for(|| b.try_wait().unwrap().is_some()));
    assert!(
        !home.path().join("serve-a.pid").exists(),
        "pid file removed on exit"
    );
}

#[test]
fn stop_without_running_serve_reports_null() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(home.path(), &["stop"], "", false);
    assert_eq!(out.status.code(), Some(0));
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reply["stopped"], Value::Null);
}

#[test]
fn deployed_probe_keeps_data_next_to_its_bin_dir() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join(".mai").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let exe = bin.join(if cfg!(windows) {
        "mai-probe.exe"
    } else {
        "mai-probe"
    });
    std::fs::copy(env!("CARGO_BIN_EXE_mai-probe"), &exe).unwrap();
    let elsewhere = root.path().join("elsewhere");
    let mut child = Command::new(&exe)
        .args(["hook", "claude"])
        .env("MAI_HOME", &elsewhere)
        .env("ZELLIJ_SESSION_NAME", "work")
        .env("ZELLIJ_PANE_ID", "5")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(STOP.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(spool_lines(&root.path().join(".mai")).len(), 1);
    assert!(
        !elsewhere.exists(),
        "MAI_HOME is ignored for a deployed probe"
    );
}
