//! Install mai-probe on a remote host (design 4.2): detect OS, CPU and
//! login shell, compare SHA-256 with the bundled binary, upload over SFTP
//! when different, then run `install-hooks`.

use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::ssh::auth::Prompter;
use crate::ssh::client::{SshError, SshSession};

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

#[derive(Debug, Clone)]
pub struct DeployOptions {
    /// Directory under the remote home; `.mai` in production (hooks are
    /// only recognised under `.mai/bin`).
    pub dir: String,
    pub install_hooks: bool,
}

impl Default for DeployOptions {
    fn default() -> Self {
        Self {
            dir: ".mai".into(),
            install_hooks: true,
        }
    }
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
    NoBinary { target: String, path: PathBuf },
    Upload(String),
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

/// Detect OS, CPU, shell and home directory of the remote host.
pub async fn detect<P: Prompter>(s: &SshSession<P>) -> Result<Remote, DeployError> {
    let uname = s.exec("uname -sm").await?;
    if uname.success()
        && let Some((os, arch)) = parse_uname(&uname.stdout_str())
    {
        let home = s.exec("printf '%s' \"$HOME\"").await?.stdout_str();
        if home.is_empty() {
            return Err(DeployError::Detect("empty $HOME".into()));
        }
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
                "not Linux/macOS (uname: {:?}) nor Windows (%OS%: {other:?})",
                uname.stdout_str().trim()
            )));
        }
    };
    let (arch_cmd, home_cmd) = match shell {
        Shell::Cmd => ("echo %PROCESSOR_ARCHITECTURE%", "echo %USERPROFILE%"),
        _ => ("$env:PROCESSOR_ARCHITECTURE", "$env:USERPROFILE"),
    };
    // A Unix other than Linux/macOS also lands here (its sh echoes %OS%
    // verbatim); report uname's output so that case is recognisable.
    let raw_arch = s.exec(arch_cmd).await?.stdout_str();
    let arch = normalize_arch(&raw_arch).ok_or_else(|| {
        DeployError::Detect(format!(
            "unknown CPU {:?} (uname: {:?})",
            raw_arch.trim(),
            uname.stdout_str().trim()
        ))
    })?;
    let home = s.exec(home_cmd).await?.stdout_str().trim().to_owned();
    if home.is_empty() {
        return Err(DeployError::Detect("empty %USERPROFILE%".into()));
    }
    Ok(Remote {
        os: Os::Windows,
        arch,
        shell,
        home,
    })
}

fn upload_err(e: impl fmt::Display) -> DeployError {
    DeployError::Upload(e.to_string())
}

/// Upload `bytes` to `<login dir>/<dir>/bin/<exe>` via SFTP (paths are
/// relative to the login directory, which is home on OpenSSH servers).
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
        if !sftp.try_exists(path.clone()).await.map_err(upload_err)? {
            sftp.create_dir(path.clone()).await.map_err(upload_err)?;
        }
    }
    let target = format!("{bin_dir}/{}", remote.exe_name());
    let tmp = format!("{target}.upload");
    let mut file = sftp.create(tmp.clone()).await.map_err(upload_err)?;
    file.write_all(bytes).await.map_err(upload_err)?;
    file.shutdown().await.map_err(upload_err)?;
    if remote.os != Os::Windows {
        let attrs = russh_sftp::protocol::FileAttributes {
            permissions: Some(0o755),
            ..Default::default()
        };
        sftp.set_metadata(tmp.clone(), attrs)
            .await
            .map_err(upload_err)?;
    }
    if sftp.try_exists(target.clone()).await.map_err(upload_err)? {
        sftp.remove_file(target.clone())
            .await
            .map_err(|e| DeployError::Upload(format!("replace {target} (probe running?): {e}")))?;
    }
    sftp.rename(tmp, target).await.map_err(upload_err)?;
    Ok(())
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
        upload(s, &remote, &opts.dir, &bytes).await?;
    }
    let hooks = if opts.install_hooks {
        let out = s.exec(&remote.invoke(&probe_path, "install-hooks")).await?;
        parse_hook_lines(&out.stdout_str())
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
