//! Where the probe keeps its data, and one `serve` per app installation.
//!
//! Hooks run inside the agent's interactive environment and `serve` runs
//! in an SSH exec environment, so environment variables differ between
//! them. Both run the same deployed binary, so the data dir is derived
//! from the binary's location when it lives in a deploy dir.
//!
//! A half-open SSH connection can leave an old `serve` running; on
//! Windows it also locks the binary so it cannot be replaced. Each
//! `serve` records its pid in `serve-<client>.pid`; a new `serve` for the
//! same client (or `mai-probe stop`) terminates the recorded process.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};

use crate::spool::sanitize_client;

/// Data dir: `<dir>` when the binary is `<dir>/bin/<exe>` and `<dir>`'s
/// name starts with `.mai` (a deploy dir such as `~/.mai`); otherwise
/// `$MAI_HOME` if set, else `<home>/.mai`.
pub fn data_dir(exe: Option<&Path>, mai_home: Option<&OsStr>, home: &Path) -> PathBuf {
    let deployed = exe.and_then(Path::parent).and_then(|bin| {
        let dir = bin.parent()?;
        let is_bin = bin.file_name() == Some(OsStr::new("bin"));
        let is_mai = dir
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(".mai"));
        (is_bin && is_mai).then(|| dir.to_path_buf())
    });
    deployed
        .or_else(|| mai_home.map(PathBuf::from))
        .unwrap_or_else(|| home.join(".mai"))
}

/// `serve-<client>.pid` (or `serve.pid` for an empty client id).
pub fn pid_file(data: &Path, client: &str) -> PathBuf {
    let client = sanitize_client(client);
    if client.is_empty() {
        data.join("serve.pid")
    } else {
        data.join(format!("serve-{client}.pid"))
    }
}

fn read_pid(file: &Path) -> Option<u32> {
    fs::read_to_string(file).ok()?.trim().parse().ok()
}

/// Is `pid` a live process whose name starts with `mai-probe`?
fn is_probe(sys: &mut System, pid: Pid) -> bool {
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid)
        .is_some_and(|p| p.name().to_string_lossy().starts_with("mai-probe"))
}

/// Terminate the `serve` recorded in `file`, unless it is this process or
/// the pid now belongs to some other program. Waits up to `wait` for it
/// to exit. Returns the pid that was stopped.
pub fn stop_recorded(file: &Path, wait: Duration) -> Option<u32> {
    let pid = read_pid(file)?;
    if pid == std::process::id() {
        return None;
    }
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    if !is_probe(&mut sys, pid) {
        return None;
    }
    sys.process(pid)?.kill();
    let deadline = Instant::now() + wait;
    while is_probe(&mut sys, pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    Some(pid.as_u32())
}

/// Record this process as the `serve` for its client.
pub fn record_self(file: &Path) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, std::process::id().to_string())?;
    fs::rename(tmp, file)
}

/// Remove `file` if it still names this process (a newer serve may have
/// replaced it).
pub fn forget_self(file: &Path) {
    if read_pid(file) == Some(std::process::id()) {
        let _ = fs::remove_file(file);
    }
}
