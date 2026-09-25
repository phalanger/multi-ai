//! The real `Connector`: SSH hosts get the probe deployed over SSH and
//! `serve` started on an exec channel; the local host gets the probe
//! copied into the home directory and `serve` started as a child process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::BufReader;

use crate::deploy::{
    DeployError, DeployOptions, HookResult, ProbeStore, Remote, Shell, deploy, hooks_result,
    normalize_arch, serve_argv, sha256_hex, stop_argv,
};
use crate::host::{Connector, HostConfig, HostKind, OpenError, Opened, Problem};
use crate::link::ProbeIo;
use crate::ssh::auth::{Prompter, SecretStore};
use crate::ssh::client::{ConnectOptions, ExecOutput, SshError, connect};
use crate::ssh::config::{HostSpec, parse_config, resolve};

/// Locale requested for `serve` so zellij output is UTF-8.
const LOCALE: &str = "en_US.UTF-8";

/// Map an SSH failure to retry-or-ask-the-user.
pub fn ssh_open_error(e: SshError) -> OpenError {
    match e {
        SshError::Connect(m) | SshError::Channel(m) => OpenError::Retry(m),
        SshError::Auth(m) => OpenError::NeedsUser(Problem::Auth(m)),
        SshError::HostKeyRejected { .. } => OpenError::NeedsUser(Problem::HostKeyRejected),
        SshError::HostKeyChanged { file, line, .. } => {
            OpenError::NeedsUser(Problem::HostKeyChanged { file, line })
        }
    }
}

/// Map a deployment failure to retry-or-ask-the-user.
pub fn deploy_open_error(e: DeployError) -> OpenError {
    match e {
        DeployError::Ssh(e) => ssh_open_error(e),
        other => OpenError::NeedsUser(Problem::Deploy(other.to_string())),
    }
}

fn hooks_outcome(out: &ExecOutput) -> Result<Vec<HookResult>, String> {
    hooks_result(out).map_err(|e| e.to_string())
}

/// Remote-style description of this machine, for choosing the probe
/// binary and building command lines.
pub fn local_remote(home: &Path) -> Option<Remote> {
    use crate::deploy::Os;
    let os = match std::env::consts::OS {
        "linux" => Os::Linux,
        "macos" => Os::MacOs,
        "windows" => Os::Windows,
        _ => return None,
    };
    Some(Remote {
        os,
        arch: normalize_arch(std::env::consts::ARCH)?,
        shell: if os == Os::Windows {
            Shell::Cmd
        } else {
            Shell::Posix
        },
        home: home.to_string_lossy().into_owned(),
    })
}

/// Put `bytes` at `exe` unless it already has them. `stop` runs first
/// when an existing, different binary is replaced (a running probe locks
/// its file on Windows). Returns whether the file was written.
pub fn place_binary(exe: &Path, bytes: &[u8], stop: impl FnOnce()) -> std::io::Result<bool> {
    let existing = std::fs::read(exe).ok();
    if existing.as_deref().map(sha256_hex) == Some(sha256_hex(bytes)) {
        return Ok(false);
    }
    if let Some(dir) = exe.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = exe.as_os_str().to_owned();
    tmp.push(".upload");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    if existing.is_some() {
        stop();
        std::fs::remove_file(exe)?;
    }
    std::fs::rename(&tmp, exe)?;
    Ok(true)
}

#[cfg(windows)]
fn no_window(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    cmd.creation_flags(0x0800_0000)
}

#[cfg(not(windows))]
fn no_window(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    cmd
}

/// Run `exe` with `argv` and collect its output.
async fn run_local(exe: &Path, argv: &[String]) -> std::io::Result<ExecOutput> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(argv).stdin(Stdio::null());
    let out = no_window(&mut cmd).output().await?;
    Ok(ExecOutput {
        status: out.status.code().and_then(|c| u32::try_from(c).ok()),
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// Everything needed to reach hosts and start their probes.
pub struct SystemConnector<P, S> {
    pub opts: ConnectOptions,
    pub prompter: Arc<P>,
    pub secrets: Arc<S>,
    pub probes: ProbeStore,
    /// `~/.ssh/config`; read on every connect so edits take effect.
    pub ssh_config: Option<PathBuf>,
    /// Local home: default keys, and where the local probe is installed.
    pub home: PathBuf,
    /// User for targets that name none.
    pub default_user: String,
    /// This app installation's id (see `mai-probe serve --client`).
    pub client: String,
    /// Deploy dir under the home directory (`.mai`).
    pub dir: String,
    /// Install agent hooks after deploying.
    pub install_hooks: bool,
    gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl<P: Prompter, S: SecretStore> SystemConnector<P, S> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        opts: ConnectOptions,
        prompter: Arc<P>,
        secrets: Arc<S>,
        probes: ProbeStore,
        ssh_config: Option<PathBuf>,
        home: PathBuf,
        default_user: String,
        client: String,
    ) -> Self {
        Self {
            opts,
            prompter,
            secrets,
            probes,
            ssh_config,
            home,
            default_user,
            client,
            dir: ".mai".to_owned(),
            install_hooks: true,
            gates: Mutex::new(HashMap::new()),
        }
    }

    /// Lock held while connecting to and deploying on host `id`, so
    /// host-key prompts and uploads for one host never overlap.
    pub fn gate(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(|p| p.into_inner());
        gates.entry(id.to_owned()).or_default().clone()
    }

    fn spec(&self, target: &str) -> Result<HostSpec, OpenError> {
        let text = match &self.ssh_config {
            None => String::new(),
            Some(p) => match std::fs::read_to_string(p) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    return Err(OpenError::NeedsUser(Problem::Config(format!(
                        "{}: {e}",
                        p.display()
                    ))));
                }
            },
        };
        let config = parse_config(&text)
            .map_err(|e| OpenError::NeedsUser(Problem::Config(e.to_string())))?;
        resolve(&config, target, &self.default_user, &self.home)
            .map_err(|e| OpenError::NeedsUser(Problem::Config(e.to_string())))
    }

    async fn open_ssh(&self, host: &HostConfig, target: &str) -> Result<Opened, OpenError> {
        let spec = self.spec(target)?;
        let gate = self.gate(&host.id);
        let guard = gate.lock().await;
        let session = connect(&spec, &self.opts, self.prompter.clone(), &*self.secrets)
            .await
            .map_err(ssh_open_error)?;
        let opts = DeployOptions {
            dir: self.dir.clone(),
            install_hooks: false,
            client: self.client.clone(),
        };
        let report = deploy(&session, &self.probes, &opts)
            .await
            .map_err(deploy_open_error)?;
        let remote = &report.remote;
        let hooks = if self.install_hooks {
            let cmd = remote.invoke(&report.probe_path, "install-hooks");
            let out = session.exec(&cmd).await.map_err(ssh_open_error)?;
            hooks_outcome(&out)
        } else {
            Ok(Vec::new())
        };
        drop(guard);
        let args = remote.serve_args(&self.client, host.zellij.as_deref());
        let env = [("LANG", LOCALE), ("LC_CTYPE", LOCALE)];
        let ch = session
            .open_exec(&remote.invoke(&report.probe_path, &args), &env)
            .await
            .map_err(ssh_open_error)?;
        let (r, w) = tokio::io::split(ch.into_stream());
        Ok(Opened {
            io: ProbeIo {
                reader: Box::new(BufReader::new(r)),
                writer: Box::new(w),
            },
            hooks,
            keep: Box::new(session),
        })
    }

    async fn open_local(&self, host: &HostConfig) -> Result<Opened, OpenError> {
        let deploy_err = |m: String| OpenError::NeedsUser(Problem::Deploy(m));
        let remote = local_remote(&self.home)
            .ok_or_else(|| deploy_err("unsupported local platform".into()))?;
        let target = remote
            .target()
            .ok_or_else(|| deploy_err("unsupported local platform".into()))?;
        let bin = self.probes.binary(target);
        let bytes =
            std::fs::read(&bin).map_err(|e| deploy_err(format!("{}: {e}", bin.display())))?;
        let exe = PathBuf::from(remote.probe_path(&self.dir));
        let gate = self.gate(&host.id);
        let guard = gate.lock().await;
        let (place_exe, stop) = (exe.clone(), stop_argv(&self.client));
        let placed = tokio::task::spawn_blocking(move || {
            place_binary(&place_exe, &bytes, || {
                let mut cmd = std::process::Command::new(&place_exe);
                cmd.args(&stop).stdin(Stdio::null()).stdout(Stdio::null());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x0800_0000);
                }
                let _ = cmd.status();
            })
        })
        .await
        .map_err(|e| deploy_err(e.to_string()))?;
        placed.map_err(|e| deploy_err(format!("{}: {e}", exe.display())))?;
        let hooks = if self.install_hooks {
            match run_local(&exe, &["install-hooks".to_owned()]).await {
                Ok(out) => hooks_outcome(&out),
                Err(e) => Err(e.to_string()),
            }
        } else {
            Ok(Vec::new())
        };
        drop(guard);
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.args(serve_argv(&self.client, host.zellij.as_deref()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = no_window(&mut cmd)
            .spawn()
            .map_err(|e| OpenError::Retry(format!("start {}: {e}", exe.display())))?;
        let (Some(out), Some(input)) = (child.stdout.take(), child.stdin.take()) else {
            return Err(OpenError::Retry("probe pipes unavailable".into()));
        };
        Ok(Opened {
            io: ProbeIo {
                reader: Box::new(BufReader::new(out)),
                writer: Box::new(input),
            },
            hooks,
            keep: Box::new(child),
        })
    }
}

impl<P: Prompter, S: SecretStore> Connector for SystemConnector<P, S> {
    async fn open(&self, host: &HostConfig) -> Result<Opened, OpenError> {
        match &host.kind {
            HostKind::Local => self.open_local(host).await,
            HostKind::Ssh { target } => self.open_ssh(host, target).await,
        }
    }
}
