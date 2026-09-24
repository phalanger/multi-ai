use std::path::Path;

use mai_probe::install::{
    CLAUDE_EVENTS, CODEX_EVENTS, InstallError, Outcome, hook_command, install_file, merge_hooks,
    remove_hooks, uninstall_file,
};
use serde_json::{Value, json};

fn cmd() -> String {
    hook_command(
        Path::new("C:\\Users\\x\\.mai\\bin\\mai-probe.exe"),
        "claude",
    )
}

fn user_hook() -> Value {
    json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "my-linter"}]})
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn backups(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".mai-bak-")
        })
        .count()
}

#[test]
fn hook_command_uses_forward_slashes_and_quotes() {
    assert_eq!(cmd(), "\"C:/Users/x/.mai/bin/mai-probe.exe\" hook claude");
}

#[test]
fn merge_adds_every_event_and_keeps_user_entries() {
    let mut doc = json!({"model": "opus", "hooks": {"PreToolUse": [user_hook()]}});
    merge_hooks(&mut doc, CLAUDE_EVENTS, &cmd()).unwrap();
    assert_eq!(doc["model"], "opus");
    for (event, with_matcher) in CLAUDE_EVENTS {
        let list = doc["hooks"][*event].as_array().unwrap();
        let ours = list.last().unwrap();
        assert_eq!(ours["hooks"][0]["command"], cmd(), "{event}");
        assert_eq!(ours.get("matcher").is_some(), *with_matcher, "{event}");
    }
    assert_eq!(doc["hooks"]["PreToolUse"][0], user_hook());
    assert_eq!(doc["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
}

#[test]
fn merge_is_idempotent() {
    let mut once = json!({});
    merge_hooks(&mut once, CODEX_EVENTS, &cmd()).unwrap();
    let mut twice = once.clone();
    merge_hooks(&mut twice, CODEX_EVENTS, &cmd()).unwrap();
    assert_eq!(once, twice);
}

#[test]
fn merge_rejects_wrong_shapes() {
    assert!(merge_hooks(&mut json!([]), CLAUDE_EVENTS, &cmd()).is_err());
    assert!(merge_hooks(&mut json!({"hooks": 3}), CLAUDE_EVENTS, &cmd()).is_err());
    let mut bad_event = json!({"hooks": {"Stop": "x"}});
    assert!(merge_hooks(&mut bad_event, CLAUDE_EVENTS, &cmd()).is_err());
}

#[test]
fn remove_drops_only_ours_and_empty_events() {
    let mut doc = json!({"hooks": {"PreToolUse": [user_hook()]}});
    merge_hooks(&mut doc, CLAUDE_EVENTS, &cmd()).unwrap();
    remove_hooks(&mut doc).unwrap();
    assert_eq!(doc, json!({"hooks": {"PreToolUse": [user_hook()]}}));
}

#[test]
fn install_creates_file_then_reports_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".codex").join("hooks.json");
    assert_eq!(
        install_file(&path, CODEX_EVENTS, &cmd(), 1).unwrap(),
        Outcome::Installed
    );
    assert_eq!(read(&path)["hooks"]["PermissionRequest"][0]["matcher"], "*");
    assert_eq!(
        install_file(&path, CODEX_EVENTS, &cmd(), 2).unwrap(),
        Outcome::Unchanged
    );
    assert_eq!(backups(path.parent().unwrap()), 0);
}

#[test]
fn install_backs_up_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, r#"{"model": "opus"}"#).unwrap();
    assert_eq!(
        install_file(&path, CLAUDE_EVENTS, &cmd(), 7).unwrap(),
        Outcome::Installed
    );
    let bak = dir.path().join("settings.json.mai-bak-7");
    assert_eq!(read(&bak), json!({"model": "opus"}));
    assert_eq!(read(&path)["model"], "opus");
}

#[test]
fn install_refuses_invalid_json_and_leaves_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, "{ not json").unwrap();
    let err = install_file(&path, CLAUDE_EVENTS, &cmd(), 1).unwrap_err();
    assert!(matches!(err, InstallError::Parse(..)), "{err}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
}

#[test]
fn uninstall_restores_user_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(
        &path,
        r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "beep"}]}]}}"#,
    )
    .unwrap();
    install_file(&path, CLAUDE_EVENTS, &cmd(), 1).unwrap();
    assert_eq!(uninstall_file(&path, 2).unwrap(), Outcome::Removed);
    assert_eq!(
        read(&path),
        json!({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "beep"}]}]}})
    );
    assert_eq!(uninstall_file(&path, 3).unwrap(), Outcome::Unchanged);
    assert_eq!(
        uninstall_file(&dir.path().join("none.json"), 4).unwrap(),
        Outcome::Unchanged
    );
}
