//! Install mai-probe on a remote host (design 4.2): detect OS, CPU and
//! login shell, compare SHA-256 with the bundled binary, upload over SFTP
//! when different, then run `install-hooks`. When an existing probe
//! differs, `mai-probe stop --client <id>` is run first (best effort) so
//! a running probe does not lock the file.

use std::fmt;
use std::path::PathBuf;

use mai_protocol::sanitize_client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::ssh::auth::Prompter;
use crate::ssh::client::{ExecOutput, SshError, SshSession};
use crate::swap::{SftpFiles, swap_in};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

/// How commands sent over `exec` are interpreted on the remote side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Posix,
    Cmd,
    PowerShell,
}

/// What `detect` learned about the remote host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub os: Os,
    /// `x86_64` or `aarch64`.
    pub arch: String,
    pub shell: Shell,
    /// Absolute home directory in the remote OS's own syntax.
    pub home: String,
}

impl Remote {
    /// Rust target triple of the probe binary this host needs.
    pub fn target(&self) -> Option<&'static str> {
        Some(match (self.os, self.arch.as_str()) {
            (Os::Linux, "x86_64") => "x86_64-unknown-linux-musl",
            (Os::Linux, "aarch64") => "aarch64-unknown-linux-musl",
            (Os::MacOs, "x86_64") => "x86_64-apple-darwin",
            (Os::MacOs, "aarch64") => "aarch64-apple-darwin",
            (Os::Windows, "x86_64") => "x86_64-pc-windows-msvc",
            _ => return None,
        })
    }

    fn exe_name(&self) -> &'static str {
        if self.os == Os::Windows {
            "mai-probe.exe"
        } else {
            "mai-probe"
        }
    }

    /// Absolute path of the probe under `<home>/<dir>/bin/`.
    pub fn probe_path(&self, dir: &str) -> String {
        if self.os == Os::Windows {
            format!(
                "{}\\{}\\bin\\{}",
                self.home,
                dir.replace('/', "\\"),
                self.exe_name()
            )
        } else {
            format!("{}/{dir}/bin/{}", self.home, self.exe_name())
        }
    }

    /// Command line that runs the program at `path` with `args`.
    pub fn invoke(&self, path: &str, args: &str) -> String {
        match self.shell {
            Shell::Posix => format!("{} {args}", sh_quote(path)),
            Shell::Cmd => format!("\"{path}\" {args}"),
            Shell::PowerShell => format!("& '{}' {args}", path.replace('\'', "''")),
        }
    }

    /// Quote one argument for this host's shell. Under cmd the value is
    /// wrapped in double quotes without escaping (Windows paths cannot
    /// contain `"`).
    pub fn quote_arg(&self, arg: &str) -> String {
        match self.shell {
            Shell::Posix => sh_quote(arg),
            Shell::Cmd => format!("\"{arg}\""),
            Shell::PowerShell => format!("'{}'", arg.replace('\'', "''")),
        }
    }

    /// Join `argv` for this host's shell, quoting every word that is not
    /// plain `[A-Za-z0-9_-]`.
    pub fn join_args(&self, argv: &[String]) -> String {
        let plain = |w: &str| {
            !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        argv.iter()
            .map(|w| {
                if plain(w) {
                    w.clone()
                } else {
                    self.quote_arg(w)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `serve_argv` joined for this host's shell.
    pub fn serve_args(&self, client: &str, zellij: Option<&str>) -> String {
        self.join_args(&serve_argv(client, zellij))
    }

    /// Command printing the SHA-256 of `path` (parse with `parse_hash`).
    pub fn hash_command(&self, path: &str) -> String {
        match (self.os, self.shell) {
            (Os::MacOs, _) => format!("shasum -a 256 {}", sh_quote(path)),
            (_, Shell::Posix) => format!("sha256sum {}", sh_quote(path)),
            (_, Shell::Cmd) => format!("certutil -hashfile \"{path}\" SHA256"),
            (_, Shell::PowerShell) => {
                format!(
                    "(Get-FileHash -Algorithm SHA256 '{}').Hash",
                    path.replace('\'', "''")
                )
            }
        }
    }
}

/// Single-quote for POSIX sh.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Parse `uname -sm` (e.g. `Darwin arm64`, `Linux x86_64`).
pub fn parse_uname(out: &str) -> Option<(Os, String)> {
    let mut it = out.split_whitespace();
    let os = match it.next()? {
        "Linux" => Os::Linux,
        "Darwin" => Os::MacOs,
        _ => return None,
    };
    Some((os, normalize_arch(it.next()?)?))
}

/// Map `uname -m` / `PROCESSOR_ARCHITECTURE` values to `x86_64`/`aarch64`.
pub fn normalize_arch(raw: &str) -> Option<String> {
    Some(
        match raw.trim().to_ascii_lowercase().as_str() {
            "x86_64" | "amd64" => "x86_64",
            "aarch64" | "arm64" => "aarch64",
            _ => return None,
        }
        .to_owned(),
    )
}

/// First 64-hex-digit token (sha256sum, shasum, certutil, Get-FileHash),
/// lowercased.
pub fn parse_hash(out: &str) -> Option<String> {
    out.split_whitespace()
        .find(|t| t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Bundled probe binaries: `<dir>/<target>/mai-probe[.exe]`.
#[derive(Debug, Clone)]
pub struct ProbeStore {
    pub dir: PathBuf,
}

impl ProbeStore {
    pub fn binary(&self, target: &str) -> PathBuf {
        let name = if target.contains("windows") {
            "mai-probe.exe"
        } else {
            "mai-probe"
        };
        self.dir.join(target).join(name)
    }
}

/// One line printed by `mai-probe install-hooks`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HookResult {
    pub agent: String,
    /// installed | removed | unchanged | skipped | error
    pub outcome: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Parse `install-hooks` output; non-JSON lines are ignored.
pub fn parse_hook_lines(out: &str) -> Vec<HookResult> {
    out.lines()
        .filter_map(|l| serde_json::from_str(l.trim()).ok())
        .collect()
}

/// Turn the finished `install-hooks` exec into its parsed results, or a
/// `DeployError::Hooks` if the command itself failed (a non-zero or
/// missing exit status is never silently treated as "no hooks installed").
pub fn hooks_result(out: &ExecOutput) -> Result<Vec<HookResult>, DeployError> {
    if !out.success() {
        return Err(DeployError::Hooks {
            status: out.status,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(parse_hook_lines(&out.stdout_str()))
}

#[derive(Debug, Clone)]
pub struct DeployOptions {
    /// Directory under the remote home; `.mai` in production (hooks are
    /// only recognised under `.mai/bin`).
    pub dir: String,
    pub install_hooks: bool,
    /// App installation id; its running `serve` is stopped before the
    /// binary is replaced (a running probe locks its file on Windows).
    pub client: String,
}

impl Default for DeployOptions {
    fn default() -> Self {
        Self {
            dir: ".mai".into(),
            install_hooks: true,
            client: String::new(),
        }
    }
}

/// Arguments of `mai-probe stop` for `client` (see `Remote::serve_args`).
pub fn stop_args(client: &str) -> String {
    stop_argv(client).join(" ")
}

/// `--client <id>` with the id reduced to `[A-Za-z0-9_-]`; nothing for
/// an empty id (PowerShell 5.1 drops empty arguments to native programs).
fn client_argv(client: &str) -> Vec<String> {
    let client = sanitize_client(client);
    if client.is_empty() {
        Vec::new()
    } else {
        vec!["--client".to_owned(), client]
    }
}

/// Arguments of `mai-probe serve` for `client`, with an optional
/// zellij path.
pub fn serve_argv(client: &str, zellij: Option<&str>) -> Vec<String> {
    let mut argv = vec!["serve".to_owned()];
    argv.extend(client_argv(client));
    if let Some(z) = zellij {
        argv.push("--zellij".to_owned());
        argv.push(z.to_owned());
    }
    argv
}

/// Arguments of `mai-probe stop` for `client`.
pub fn stop_argv(client: &str) -> Vec<String> {
    let mut argv = vec!["stop".to_owned()];
    argv.extend(client_argv(client));
    argv
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployReport {
    pub remote: Remote,
    pub probe_path: String,
    /// False when the remote binary already matched.
    pub uploaded: bool,
    pub hooks: Vec<HookResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployError {
    Detect(String),
    NoBinary {
        target: String,
        path: PathBuf,
    },
    Upload(String),
    /// `install-hooks` exited with a non-zero (or missing) status.
    Hooks {
        status: Option<u32>,
        stderr: String,
    },
    Ssh(SshError),
}

impl fmt::Display for DeployError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Detect(m) => write!(f, "detect remote platform: {m}"),
            Self::NoBinary { target, path } => {
                write!(f, "no probe for {target} (expected {})", path.display())
            }
            Self::Upload(m) => write!(f, "upload probe: {m}"),
            Self::Hooks { status, stderr } => {
                write!(f, "install-hooks failed (status {status:?}): {stderr}")
            }
            Self::Ssh(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DeployError {}

impl From<SshError> for DeployError {
    fn from(e: SshError) -> Self {
        Self::Ssh(e)
    }
}

/// Start and end markers around detection output, so text a shell's
/// startup files print is not mistaken for it (B11).
pub const BEGIN_MARK: &str = "MAI-DETECT-BEGIN";
pub const END_MARK: &str = "MAI-DETECT-END";

/// One POSIX command printing `uname -sm` and `$HOME` between markers.
/// It is a flat `&&` chain, with no braces, so that fish and PowerShell
/// on Unix (which cannot parse `{ ...; }`) still run it. The markers are
/// printed only when `uname` works: Windows PowerShell 5.1 rejects `&&`
/// as a syntax error and cmd fails on the `/dev/null` redirect, so
/// neither prints a marked block (a plain `echo` of the markers would
/// succeed under PowerShell even though `uname` fails). Other shells run
/// the chain as written.
pub fn posix_detect_command() -> String {
    format!(
        "uname -sm >/dev/null && echo {BEGIN_MARK} && uname -sm && printf '%s\\n' \"$HOME\" && echo {END_MARK}"
    )
}

/// Detection commands per Windows shell: CPU and home between markers.
pub fn windows_detect_command(shell: Shell) -> String {
    match shell {
        Shell::Cmd => format!(
            "echo {BEGIN_MARK}& echo %PROCESSOR_ARCHITECTURE%& echo %USERPROFILE%& echo {END_MARK}"
        ),
        _ => format!(
            "echo {BEGIN_MARK}; $env:PROCESSOR_ARCHITECTURE; $env:USERPROFILE; echo {END_MARK}"
        ),
    }
}

/// The trimmed lines between the first `BEGIN_MARK` line and the next
/// `END_MARK` line, or `None` if the markers are missing.
pub fn marked_lines(out: &str) -> Option<Vec<String>> {
    let mut lines = out.lines().map(str::trim);
    lines.find(|l| *l == BEGIN_MARK)?;
    let mut inner = Vec::new();
    for l in lines {
        if l == END_MARK {
            return Some(inner);
        }
        inner.push(l.to_owned());
    }
    None
}

/// OS, CPU and home from `posix_detect_command` output. `Ok(None)` when
/// the output is not from a POSIX shell (no markers, an empty marked
/// block, or a `uname` naming MINGW, MSYS or CYGWIN: a Windows host whose
/// login shell found Git's `uname` on PATH).
pub fn parse_posix_detect(out: &str) -> Result<Option<(Os, String, String)>, DeployError> {
    let Some(lines) = marked_lines(out) else {
        return Ok(None);
    };
    if lines.is_empty() {
        return Ok(None);
    }
    let [uname, home] = lines.as_slice() else {
        return Err(DeployError::Detect(format!(
            "unexpected detection output: {lines:?}"
        )));
    };
    let kernel = uname.to_ascii_uppercase();
    if ["MINGW", "MSYS", "CYGWIN"]
        .iter()
        .any(|p| kernel.starts_with(p))
    {
        return Ok(None);
    }
    let (os, arch) = parse_uname(uname)
        .ok_or_else(|| DeployError::Detect(format!("unsupported system (uname: {uname:?})")))?;
    if home.is_empty() {
        return Err(DeployError::Detect("empty $HOME".into()));
    }
    Ok(Some((os, arch, home.clone())))
}

/// CPU and home from `windows_detect_command` output. Under cmd an unset
/// variable is echoed back as `%NAME%`, which counts as missing (B12).
pub fn parse_windows_detect(out: &str) -> Result<(String, String), DeployError> {
    let lines = marked_lines(out)
        .ok_or_else(|| DeployError::Detect(format!("unexpected detection output: {out:?}")))?;
    let [raw_arch, home] = lines.as_slice() else {
        return Err(DeployError::Detect(format!(
            "unexpected detection output: {lines:?}"
        )));
    };
    let arch = normalize_arch(raw_arch)
        .ok_or_else(|| DeployError::Detect(format!("unknown CPU {raw_arch:?}")))?;
    if home.is_empty() || home.eq_ignore_ascii_case("%USERPROFILE%") {
        return Err(DeployError::Detect("USERPROFILE is not set".into()));
    }
    Ok((arch, home.clone()))
}

/// Detect OS, CPU, shell and home directory of the remote host.
pub async fn detect<P: Prompter>(s: &SshSession<P>) -> Result<Remote, DeployError> {
    let posix = s.exec(&posix_detect_command()).await?;
    if let Some((os, arch, home)) = parse_posix_detect(&posix.stdout_str())? {
        return Ok(Remote {
            os,
            arch,
            shell: Shell::Posix,
            home,
        });
    }
    // Windows: cmd expands %OS%, PowerShell prints it verbatim.
    let shell = match s.exec("echo %OS%").await?.stdout_str().trim() {
        "Windows_NT" => Shell::Cmd,
        "%OS%" => Shell::PowerShell,
        other => {
            return Err(DeployError::Detect(format!(
                "neither a POSIX shell nor Windows (%OS%: {other:?})"
            )));
        }
    };
    let out = s.exec(&windows_detect_command(shell)).await?;
    let (arch, home) = parse_windows_detect(&out.stdout_str())?;
    Ok(Remote {
        os: Os::Windows,
        arch,
        shell,
        home,
    })
}

/// An upload step failed. A timeout (a slow link) is a network problem and
/// is retried like other network errors (design 3.4, B34); anything else
/// needs the user.
pub fn upload_error(e: impl Into<std::io::Error>) -> DeployError {
    let e: std::io::Error = e.into();
    if e.kind() == std::io::ErrorKind::TimedOut {
        DeployError::Ssh(SshError::Connect(format!("upload timed out: {e}")))
    } else {
        DeployError::Upload(e.to_string())
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Upload `bytes` to `<login dir>/<dir>/bin/<exe>` via SFTP (paths are
/// relative to the login directory, which is home on OpenSSH servers).
/// The file is written to `<exe>.upload`, its SHA-256 is checked on the
/// host (B18), and it is swapped in with the old binary moved aside
/// rather than deleted (B19).
async fn upload<P: Prompter>(
    s: &SshSession<P>,
    remote: &Remote,
    dir: &str,
    bytes: &[u8],
) -> Result<(), DeployError> {
    let sftp = s.sftp().await?;
    let bin_dir = format!("{dir}/bin");
    let mut path = String::new();
    for part in bin_dir.split('/') {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(part);
        if !sftp.try_exists(path.clone()).await.map_err(upload_error)? {
            sftp.create_dir(path.clone()).await.map_err(upload_error)?;
        }
    }
    let target = format!("{bin_dir}/{}", remote.exe_name());
    let tmp = format!("{target}.upload");
    let mut file = sftp.create(tmp.clone()).await.map_err(upload_error)?;
    file.write_all(bytes).await.map_err(upload_error)?;
    file.shutdown().await.map_err(upload_error)?;
    if remote.os != Os::Windows {
        let attrs = russh_sftp::protocol::FileAttributes {
            permissions: Some(0o755),
            ..Default::default()
        };
        sftp.set_metadata(tmp.clone(), attrs)
            .await
            .map_err(upload_error)?;
    }
    let tmp_abs = format!("{}.upload", remote.probe_path(dir));
    let out = s.exec(&remote.hash_command(&tmp_abs)).await?;
    let Some(written) = parse_hash(&out.stdout_str()) else {
        let _ = sftp.remove_file(tmp).await;
        return Err(DeployError::Upload(format!(
            "cannot compute SHA-256 of {tmp_abs} on the host (exit {:?}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    };
    if written != sha256_hex(bytes) {
        let _ = sftp.remove_file(tmp).await;
        return Err(DeployError::Upload(format!(
            "uploaded file does not match (SHA-256 {written})"
        )));
    }
    swap_in(&SftpFiles(&sftp), &bin_dir, remote.exe_name(), now_ms())
        .await
        .map_err(|e| DeployError::Upload(format!("replace {target}: {e}")))
}

/// Make sure the right probe is on the host, then (optionally) install
/// agent hooks. Uploads only when the remote SHA-256 differs.
pub async fn deploy<P: Prompter>(
    s: &SshSession<P>,
    store: &ProbeStore,
    opts: &DeployOptions,
) -> Result<DeployReport, DeployError> {
    let remote = detect(s).await?;
    let target = remote.target().ok_or_else(|| {
        DeployError::Detect(format!("unsupported {:?} {}", remote.os, remote.arch))
    })?;
    let local = store.binary(target);
    let bytes = std::fs::read(&local).map_err(|_| DeployError::NoBinary {
        target: target.to_owned(),
        path: local.clone(),
    })?;
    let probe_path = remote.probe_path(&opts.dir);
    let remote_hash = parse_hash(
        &s.exec(&remote.hash_command(&probe_path))
            .await?
            .stdout_str(),
    );
    let uploaded = remote_hash.as_deref() != Some(sha256_hex(&bytes).as_str());
    if uploaded {
        if remote_hash.is_some() {
            // Best effort: an older probe may not know `stop`.
            let _ = s
                .exec(&remote.invoke(&probe_path, &stop_args(&opts.client)))
                .await;
        }
        upload(s, &remote, &opts.dir, &bytes).await?;
    }
    let hooks = if opts.install_hooks {
        let out = s.exec(&remote.invoke(&probe_path, "install-hooks")).await?;
        hooks_result(&out)?
    } else {
        Vec::new()
    };
    Ok(DeployReport {
        remote,
        probe_path,
        uploaded,
        hooks,
    })
}
