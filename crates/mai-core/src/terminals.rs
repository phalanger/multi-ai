//! The real `TermTransport`s: an SSH session dedicated to terminals (a
//! PTY channel per terminal), or local PTYs for this machine.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::deploy::Remote;
use crate::host::{OpenError, Problem};
use crate::pty::{spawn_local, PtyIo};
use crate::ssh::auth::Prompter;
use crate::ssh::client::SshSession;
use crate::term::{attach_argv, remote_attach_command, AttachSpec, TermTransport, LOCALE_ENV};

/// Terminal connection of one host.
pub enum SystemTerminals<P: Prompter> {
    /// Its own SSH session (the probe has another one), what `detect`
    /// learned about the host's shell, and the zellij found on the host
    /// (used when neither the host config nor the probe names one).
    Ssh {
        session: SshSession<P>,
        remote: Remote,
        found: Option<String>,
    },
    /// This machine: each terminal is a local PTY.
    Local,
}

/// Directories searched for zellij after PATH, relative to `$HOME` when
/// they start with `~/` (the same list the probe uses).
pub const ZELLIJ_DIRS: [&str; 4] = [
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "~/.cargo/bin",
    "~/.local/bin",
];

/// POSIX `sh` script that prints where zellij is: PATH, then
/// `ZELLIJ_DIRS`, then the user's login shell (whose profile may extend
/// PATH). Non-interactive SSH sessions often lack the directories a
/// package manager adds (e.g. `/opt/homebrew/bin` on macOS).
pub fn find_zellij_script() -> String {
    let dirs: Vec<String> = ZELLIJ_DIRS
        .iter()
        .map(|d| match d.strip_prefix("~/") {
            Some(rest) => format!("\"$HOME/{rest}\""),
            None => (*d).to_owned(),
        })
        .collect();
    format!(
        "command -v zellij || for d in {}; do if [ -x \"$d/zellij\" ]; then echo \"$d/zellij\"; exit 0; fi; done; exec \"${{SHELL:-sh}}\" -lc \"command -v zellij\"",
        dirs.join(" ")
    )
}

/// `find_zellij_script` run by `sh`, whatever the login shell is.
pub fn find_zellij_command() -> String {
    format!("sh -c '{}'", find_zellij_script())
}

/// The path printed by `find_zellij_command`: the last line that is an
/// absolute path (a login shell's startup files may print other lines).
pub fn found_zellij(out: &str) -> Option<String> {
    out.lines()
        .map(str::trim)
        .rfind(|l| l.starts_with('/') && !l.contains(' '))
        .map(str::to_owned)
}

/// The zellij to run locally when none is known: `zellij` if it is on
/// PATH, else the first of `ZELLIJ_DIRS` (under `home`) that has it.
pub fn local_zellij(path_env: Option<&std::ffi::OsStr>, home: Option<&Path>) -> OsString {
    let exe = if cfg!(windows) {
        "zellij.exe"
    } else {
        "zellij"
    };
    let on_path = path_env
        .into_iter()
        .flat_map(std::env::split_paths)
        .any(|d| d.join(exe).is_file());
    if on_path || cfg!(windows) {
        return OsString::from("zellij");
    }
    ZELLIJ_DIRS
        .iter()
        .filter_map(|d| match d.strip_prefix("~/") {
            Some(rest) => home.map(|h| h.join(rest)),
            None => Some(PathBuf::from(d)),
        })
        .map(|d| d.join(exe))
        .find(|p| p.is_file())
        .map_or_else(|| OsString::from("zellij"), PathBuf::into_os_string)
}

/// UTF-8 locale used when the environment has none: `C.UTF-8` exists on
/// every current Linux; macOS has `en_US.UTF-8`.
pub const FALLBACK_LOCALE: &str = if cfg!(target_os = "linux") {
    "C.UTF-8"
} else {
    "en_US.UTF-8"
};

/// Whether the locale in effect (`LC_ALL`, else `LC_CTYPE`, else `LANG`)
/// is UTF-8.
fn has_utf8_locale(var: &impl Fn(&str) -> Option<String>) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|k| var(k).filter(|v| !v.is_empty()))
        .next()
        .is_some_and(|v| {
            let v = v.to_ascii_lowercase();
            v.contains("utf-8") || v.contains("utf8")
        })
}

/// Environment for a local `zellij attach`, given the app's environment
/// (`var`): a terminal type, and a UTF-8 locale only when the environment
/// has none (a GUI app may have neither). A user's own UTF-8 locale (e.g.
/// `zh_CN.UTF-8`) is kept. The spawned process only adds variables, so an
/// inherited non-empty `LC_ALL` would override `LANG` and `LC_CTYPE`: when
/// the locale is not UTF-8 and `LC_ALL` is set, it is overridden as well.
pub fn local_env_with(var: impl Fn(&str) -> Option<String>) -> Vec<(&'static str, &'static str)> {
    let mut env = vec![("TERM", "xterm-256color")];
    if !cfg!(windows) && !has_utf8_locale(&var) {
        env.extend([("LANG", FALLBACK_LOCALE), ("LC_CTYPE", FALLBACK_LOCALE)]);
        if var("LC_ALL").is_some_and(|v| !v.is_empty()) {
            env.push(("LC_ALL", FALLBACK_LOCALE));
        }
    }
    env
}

/// `local_env_with` for this process's environment.
pub fn local_env() -> Vec<(&'static str, &'static str)> {
    local_env_with(|k| std::env::var(k).ok())
}

impl<P: Prompter> TermTransport for SystemTerminals<P> {
    async fn attach(
        &mut self,
        zellij: Option<&str>,
        spec: &AttachSpec,
    ) -> Result<PtyIo, OpenError> {
        match self {
            Self::Ssh {
                session,
                remote,
                found,
            } => {
                let zellij = zellij.or(found.as_deref());
                let (name, create) = (spec.session.as_str(), spec.create);
                session
                    .open_pty(spec.size, &LOCALE_ENV, |accepted| {
                        remote_attach_command(remote, zellij, name, create, accepted)
                    })
                    .await
                    .map_err(crate::connect::ssh_open_error)
            }
            Self::Local => {
                let program = match zellij {
                    Some(z) => OsString::from(z),
                    None => {
                        let home = std::env::var_os("HOME").map(PathBuf::from);
                        local_zellij(std::env::var_os("PATH").as_deref(), home.as_deref())
                    }
                };
                let argv = attach_argv(&spec.session, spec.create);
                spawn_local(&program, &argv, &local_env(), spec.size).map_err(|e| {
                    OpenError::NeedsUser(Problem::Config(format!(
                        "cannot start {}: {e}",
                        program.to_string_lossy()
                    )))
                })
            }
        }
    }

    fn alive(&self) -> bool {
        match self {
            Self::Ssh { session, .. } => !session.is_closed(),
            Self::Local => true,
        }
    }
}
