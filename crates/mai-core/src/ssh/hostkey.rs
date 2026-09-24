//! Server host-key verification against OpenSSH `known_hosts` files.

use std::path::{Path, PathBuf};

use russh::keys::{HashAlg, PublicKey};

/// Result of looking a host key up in the known_hosts files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// Some file records exactly this key for the host.
    Known,
    /// No file has a key of this algorithm for the host.
    Unknown,
    /// A file records a different key of the same algorithm: refuse.
    Changed { file: PathBuf, line: usize },
}

/// `SHA256:<base64>` as printed by `ssh-keygen -l`.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// Check `key` for `host:port` in every file. A changed key in any file
/// wins over a match elsewhere; missing or unreadable files are skipped.
pub fn check(files: &[PathBuf], host: &str, port: u16, key: &PublicKey) -> HostKeyStatus {
    let mut known = false;
    for file in files.iter().filter(|f| f.is_file()) {
        match russh::keys::check_known_hosts_path(host, port, key, file) {
            Ok(true) => known = true,
            Ok(false) => {}
            Err(russh::keys::Error::KeyChanged { line }) => {
                return HostKeyStatus::Changed {
                    file: file.clone(),
                    line,
                };
            }
            Err(_) => {}
        }
    }
    if known {
        HostKeyStatus::Known
    } else {
        HostKeyStatus::Unknown
    }
}

/// Append `key` for `host:port` to `file` (created with parents if missing).
pub fn learn(file: &Path, host: &str, port: u16, key: &PublicKey) -> Result<(), String> {
    russh::keys::known_hosts::learn_known_hosts_path(host, port, key, file)
        .map_err(|e| format!("{}: {e}", file.display()))
}
