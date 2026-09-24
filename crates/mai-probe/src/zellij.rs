//! zellij CLI access: locating the binary, parsing its output, and the
//! `Zellij` trait that `serve` uses (faked in tests).

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use mai_protocol::{PaneInfo, SessionInfo};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZellijError(pub String);

impl fmt::Display for ZellijError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ZellijError {}

/// Operations `serve` needs from zellij.
pub trait Zellij {
    fn sessions(&self) -> Result<Vec<SessionInfo>, ZellijError>;
    fn panes(&self, session: &str) -> Result<Vec<PaneInfo>, ZellijError>;
    fn dump_screen(&self, session: &str, pane_id: u32) -> Result<String, ZellijError>;
    fn paste(&self, session: &str, pane_id: u32, text: &str) -> Result<(), ZellijError>;
    fn focus(&self, session: &str, tab_id: u32, pane_id: u32) -> Result<(), ZellijError>;
}

/// Parse `zellij list-sessions -n`. Lines look like
/// `work [Created 2days ago] (current)` or
/// `old [Created 1h ago] (EXITED - attach to resurrect)`.
pub fn parse_sessions(text: &str) -> Vec<SessionInfo> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let name = l.split(" [Created").next()?.trim();
            (!name.is_empty()).then(|| SessionInfo {
                name: name.to_owned(),
                exited: l.contains("(EXITED"),
            })
        })
        .collect()
}

/// What zellij prints (and possibly fails with) when no session exists.
const NO_SESSIONS: &str = "No active zellij sessions";

/// Interpret `zellij list-sessions -n` output. "No active sessions" is an
/// empty list whatever the exit status, so the app learns that the last
/// session is gone.
pub fn sessions_from_output(
    success: bool,
    stdout: &str,
    stderr: &str,
) -> Result<Vec<SessionInfo>, ZellijError> {
    if stdout.contains(NO_SESSIONS) || stderr.contains(NO_SESSIONS) {
        Ok(Vec::new())
    } else if success {
        Ok(parse_sessions(stdout))
    } else {
        Err(ZellijError(format!(
            "zellij list-sessions failed: {}",
            stderr.trim()
        )))
    }
}

#[derive(Deserialize)]
struct RawPane {
    id: u32,
    is_plugin: bool,
    #[serde(default)]
    title: String,
    #[serde(default)]
    exited: bool,
    #[serde(default)]
    tab_id: u32,
    #[serde(default)]
    tab_name: String,
    terminal_command: Option<String>,
    /// Foreground command currently running in the pane (zellij >= 0.44
    /// with `list-panes -a`); absent while the shell is idle.
    pane_command: Option<String>,
}

/// Parse `zellij action list-panes -a -j`, keeping terminal panes only.
/// `command` is the running foreground command when known, else the
/// command the pane was started with.
pub fn parse_panes(json: &str) -> Result<Vec<PaneInfo>, serde_json::Error> {
    let raw: Vec<RawPane> = serde_json::from_str(json)?;
    Ok(raw
        .into_iter()
        .filter(|p| !p.is_plugin)
        .map(|p| PaneInfo {
            id: p.id,
            tab_id: p.tab_id,
            tab_name: p.tab_name,
            title: p.title,
            command: p.pane_command.or(p.terminal_command),
            exited: p.exited,
        })
        .collect())
}

/// Parse `zellij --version` output (`zellij 0.44.3`).
pub fn parse_version(text: &str) -> Option<String> {
    text.split_whitespace().nth(1).map(str::to_owned)
}

const EXE_NAMES: &[&str] = &["zellij", "zellij.exe"];

fn exe_in(dir: &Path) -> Option<PathBuf> {
    EXE_NAMES.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// Locate zellij: explicit path, then `PATH`, then common install dirs.
/// Non-interactive SSH sessions often lack Homebrew/cargo dirs in `PATH`.
pub fn find_zellij(
    explicit: Option<&Path>,
    path_env: Option<&OsStr>,
    home: &Path,
) -> Option<PathBuf> {
    find_zellij_in(explicit, path_env, &default_dirs(home))
}

/// Common install dirs checked after `PATH`, in order.
pub fn default_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".cargo").join("bin"),
        home.join(".local").join("bin"),
    ]
}

/// `find_zellij` with an explicit candidate-dir list (testable without
/// depending on what the host has installed).
pub fn find_zellij_in(
    explicit: Option<&Path>,
    path_env: Option<&OsStr>,
    dirs: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.is_file().then(|| p.to_path_buf());
    }
    let from_path = path_env
        .into_iter()
        .flat_map(std::env::split_paths)
        .find_map(|d| exe_in(&d));
    from_path.or_else(|| dirs.iter().find_map(|d| exe_in(d)))
}

/// Last resort on Unix: ask the user's login shell.
pub fn find_via_login_shell(shell: &OsStr) -> Option<PathBuf> {
    let out = Command::new(shell)
        .args(["-lc", "command -v zellij"])
        .output()
        .ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    let path = PathBuf::from(path.trim());
    (out.status.success() && path.is_file()).then_some(path)
}

/// `Zellij` backed by the real CLI.
pub struct CliZellij {
    exe: PathBuf,
}

impl CliZellij {
    pub fn new(exe: PathBuf) -> Self {
        Self { exe }
    }

    /// Run zellij; the output is returned whatever the exit status.
    fn run_raw<I, S>(&self, args: I) -> Result<Output, ZellijError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new(&self.exe)
            .args(args)
            .output()
            .map_err(|e| ZellijError(format!("spawn {}: {e}", self.exe.display())))
    }

    fn run<I, S>(&self, args: I) -> Result<String, ZellijError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.run_raw(args)?;
        if !out.status.success() {
            return Err(ZellijError(format!(
                "zellij exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn action(&self, session: &str, rest: &[&str]) -> Result<String, ZellijError> {
        let mut args = vec!["--session", session, "action"];
        args.extend_from_slice(rest);
        self.run(args)
    }

    pub fn version(&self) -> Result<String, ZellijError> {
        let out = self.run(["--version"])?;
        parse_version(&out).ok_or_else(|| ZellijError(format!("bad version output: {out}")))
    }
}

fn pane_arg(pane_id: u32) -> String {
    format!("terminal_{pane_id}")
}

impl Zellij for CliZellij {
    fn sessions(&self) -> Result<Vec<SessionInfo>, ZellijError> {
        let out = self.run_raw(["list-sessions", "-n"])?;
        sessions_from_output(
            out.status.success(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        )
    }

    fn panes(&self, session: &str) -> Result<Vec<PaneInfo>, ZellijError> {
        let out = self.action(session, &["list-panes", "-a", "-j"])?;
        parse_panes(&out).map_err(|e| ZellijError(format!("list-panes json: {e}")))
    }

    fn dump_screen(&self, session: &str, pane_id: u32) -> Result<String, ZellijError> {
        self.action(session, &["dump-screen", "-p", &pane_arg(pane_id)])
    }

    fn paste(&self, session: &str, pane_id: u32, text: &str) -> Result<(), ZellijError> {
        self.action(session, &["paste", "-p", &pane_arg(pane_id), text])
            .map(drop)
    }

    fn focus(&self, session: &str, tab_id: u32, pane_id: u32) -> Result<(), ZellijError> {
        self.action(session, &["go-to-tab-by-id", &tab_id.to_string()])?;
        self.action(session, &["focus-pane-id", &pane_arg(pane_id)])
            .map(drop)
    }
}
