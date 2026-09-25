//! Merge-style install/uninstall of our hook entries into agent configs:
//! Claude Code `~/.claude/settings.json` and Codex `~/.codex/hooks.json`.
//! Both use the same shape:
//! `{"hooks": {"<Event>": [{"matcher": "*", "hooks": [{"type": "command",
//! "command": "..."}]}]}}`. Entries we own are recognised by `MARKER` in
//! their command; everything else is left untouched.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

/// Substring present in every hook command we install.
pub const MARKER: &str = ".mai/bin/mai-probe";

/// (event, needs a `"*"` matcher). Tool events take a matcher.
pub const CLAUDE_EVENTS: &[(&str, bool)] = &[
    ("SessionStart", false),
    ("UserPromptSubmit", false),
    ("PreToolUse", true),
    ("PostToolUse", true),
    ("Notification", false),
    ("Stop", false),
    ("SessionEnd", false),
];

pub const CODEX_EVENTS: &[(&str, bool)] = &[
    ("SessionStart", false),
    ("UserPromptSubmit", false),
    ("PreToolUse", true),
    ("PostToolUse", true),
    ("PermissionRequest", true),
    ("Stop", false),
    ("Interrupt", false),
    ("SessionEnd", false),
];

#[derive(Debug)]
pub enum InstallError {
    Io(PathBuf, io::Error),
    Parse(PathBuf, serde_json::Error),
    Shape(PathBuf, &'static str),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(p, e) => write!(f, "{}: {e}", p.display()),
            Self::Parse(p, e) => write!(f, "{}: invalid JSON: {e}", p.display()),
            Self::Shape(p, what) => write!(f, "{}: {what}", p.display()),
        }
    }
}

impl std::error::Error for InstallError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Installed,
    Removed,
    Unchanged,
}

impl Outcome {
    /// Name used in `install-hooks` / `uninstall-hooks` output.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Installed => "installed",
            Self::Removed => "removed",
            Self::Unchanged => "unchanged",
        }
    }
}

/// Hook command for `agent`, e.g. `"C:/Users/x/.mai/bin/mai-probe.exe" hook claude`.
/// Backslashes are normalised to `/` so `MARKER` matches on every OS.
pub fn hook_command(probe_exe: &Path, agent: &str) -> String {
    let exe = probe_exe.to_string_lossy().replace('\\', "/");
    format!("\"{exe}\" hook {agent}")
}

/// Hooks are recognised as ours only by `MARKER` in the command, so the
/// probe must run from an absolute path under `.mai/bin`. Otherwise
/// uninstall could not find the entries and a reinstall would duplicate
/// them. Not canonicalized: Windows `\\?\` paths would break the command.
pub fn check_probe_exe(exe: &Path) -> Result<(), &'static str> {
    if !exe.is_absolute() {
        return Err("probe path is not absolute");
    }
    if !hook_command(exe, "x").contains(MARKER) {
        return Err("probe is not under .mai/bin; hooks would not be recognised");
    }
    Ok(())
}

fn is_ours(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|c| c.contains(MARKER))
}

/// Remove our hook objects from every group of one event's list. A group
/// is dropped only when that leaves its `hooks` array empty, so a user
/// hook sharing a group with ours survives.
fn strip_ours(list: &mut Vec<Value>) {
    list.retain_mut(|group| {
        let Some(hs) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = hs.len();
        hs.retain(|h| !is_ours(h));
        hs.len() == before || !hs.is_empty()
    });
}

fn hooks_table(doc: &mut Value) -> Result<&mut Map<String, Value>, &'static str> {
    let root = doc
        .as_object_mut()
        .ok_or("top level is not a JSON object")?;
    root.entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("\"hooks\" is not a JSON object")
}

/// Replace our entries in `doc` with one entry per event. Idempotent.
pub fn merge_hooks(
    doc: &mut Value,
    events: &[(&str, bool)],
    command: &str,
) -> Result<(), &'static str> {
    let hooks = hooks_table(doc)?;
    for (event, with_matcher) in events {
        let list = hooks
            .entry(*event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or("a hook event is not a JSON array")?;
        strip_ours(list);
        let mut entry = json!({"hooks": [{"type": "command", "command": command}]});
        if *with_matcher {
            entry["matcher"] = json!("*");
        }
        list.push(entry);
    }
    Ok(())
}

/// Remove our hooks; drop groups and event keys that become empty.
pub fn remove_hooks(doc: &mut Value) -> Result<(), &'static str> {
    let Some(root) = doc.as_object_mut() else {
        return Err("top level is not a JSON object");
    };
    let Some(hooks) = root.get_mut("hooks") else {
        return Ok(());
    };
    let hooks = hooks
        .as_object_mut()
        .ok_or("\"hooks\" is not a JSON object")?;
    for list in hooks.values_mut() {
        if let Some(list) = list.as_array_mut() {
            strip_ours(list);
        }
    }
    hooks.retain(|_, list| list.as_array().is_none_or(|l| !l.is_empty()));
    Ok(())
}

fn read_doc(path: &Path) -> Result<Value, InstallError> {
    match fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => serde_json::from_str(&text).map_err(|e| InstallError::Parse(path.into(), e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(InstallError::Io(path.into(), e)),
    }
}

/// Most symlink hops followed when resolving a config path.
const MAX_LINKS: usize = 16;

/// File that a write to `path` must replace: the final symlink target
/// when `path` is a symlink (so the link itself survives), else `path`.
/// A relative link target is taken relative to the link's directory.
fn write_target(path: &Path) -> io::Result<PathBuf> {
    let mut target = path.to_path_buf();
    for _ in 0..MAX_LINKS {
        match fs::symlink_metadata(&target) {
            Ok(m) if m.file_type().is_symlink() => {
                let link = fs::read_link(&target)?;
                target = match target.parent() {
                    Some(dir) if link.is_relative() => dir.join(link),
                    _ => link,
                };
            }
            _ => return Ok(target),
        }
    }
    Err(io::Error::other("too many levels of symbolic links"))
}

/// Back up the existing file (if any) as `<name>.mai-bak-<ms>`, then
/// write `doc` via a temp file + rename. A symlinked config is written
/// through to its target, and the target's permissions are kept.
fn write_doc(path: &Path, doc: &Value, now_ms: u64) -> Result<(), InstallError> {
    let io_err = |e| InstallError::Io(path.into(), e);
    let target = write_target(path).map_err(io_err)?;
    let perms = match fs::metadata(&target) {
        Ok(m) => Some(m.permissions()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(io_err(e)),
    };
    if perms.is_some() {
        let mut bak = target.as_os_str().to_owned();
        bak.push(format!(".mai-bak-{now_ms}"));
        fs::copy(&target, PathBuf::from(bak)).map_err(io_err)?;
    } else if let Some(dir) = target.parent() {
        fs::create_dir_all(dir).map_err(io_err)?;
    }
    let mut tmp = target.as_os_str().to_owned();
    tmp.push(format!(".mai-tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let mut text = serde_json::to_string_pretty(doc).expect("Value serializes");
    text.push('\n');
    fs::write(&tmp, text).map_err(io_err)?;
    if let Some(perms) = perms {
        fs::set_permissions(&tmp, perms).map_err(io_err)?;
    }
    fs::rename(&tmp, &target).map_err(io_err)
}

fn apply(
    path: &Path,
    now_ms: u64,
    changed: Outcome,
    edit: impl FnOnce(&mut Value) -> Result<(), &'static str>,
) -> Result<Outcome, InstallError> {
    let before = read_doc(path)?;
    let mut after = before.clone();
    edit(&mut after).map_err(|what| InstallError::Shape(path.into(), what))?;
    if after == before {
        return Ok(Outcome::Unchanged);
    }
    write_doc(path, &after, now_ms)?;
    Ok(changed)
}

/// Install our hooks into the config file at `path` (created if missing).
pub fn install_file(
    path: &Path,
    events: &[(&str, bool)],
    command: &str,
    now_ms: u64,
) -> Result<Outcome, InstallError> {
    apply(path, now_ms, Outcome::Installed, |doc| {
        merge_hooks(doc, events, command)
    })
}

/// Remove our hooks from the config file at `path`.
pub fn uninstall_file(path: &Path, now_ms: u64) -> Result<Outcome, InstallError> {
    if !path.exists() {
        return Ok(Outcome::Unchanged);
    }
    apply(path, now_ms, Outcome::Removed, remove_hooks)
}
