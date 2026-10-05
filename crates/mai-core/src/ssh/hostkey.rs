//! Server host-key verification against OpenSSH `known_hosts` files.
//!
//! Files are parsed line by line and leniently: a line that cannot be
//! parsed (bad base64, unsupported key type, invalid UTF-8) is skipped
//! like OpenSSH does, instead of making the whole file unusable. Host
//! patterns support `*`/`?` globs, `!` negation, `[host]:port` and hashed
//! names (`|1|salt|hash`, HMAC-SHA1). `@revoked` entries refuse a key;
//! `@cert-authority` entries are ignored (host certificates are refused).

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use hmac::{Hmac, KeyInit, Mac};
use russh::keys::ssh_key::known_hosts::{HostPatterns, KnownHosts, Marker};
use russh::keys::{HashAlg, PublicKey};
use sha1::Sha1;

/// Result of looking a host key up in the known_hosts files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// Some file records exactly this key for the host.
    Known,
    /// No file has a key of this algorithm for the host.
    Unknown,
    /// A file records a different key of the same algorithm: refuse.
    Changed { file: PathBuf, line: usize },
    /// A file marks this key `@revoked` for the host: refuse.
    Revoked { file: PathBuf, line: usize },
    /// A file exists but cannot be read (I/O error): refuse rather than
    /// treat it as missing.
    Unreadable { file: PathBuf, error: String },
}

/// `SHA256:<base64>` as printed by `ssh-keygen -l`.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// The name OpenSSH records for `host:port`: the lowercased host, or
/// `[host]:port` when the port is not 22.
pub fn host_name(host: &str, port: u16) -> String {
    let host = host.to_ascii_lowercase();
    if port == 22 {
        host
    } else {
        format!("[{host}]:{port}")
    }
}

/// Glob match with `*` (any run) and `?` (one character).
fn glob(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|i| glob(rest, &text[i..])),
        Some(('?', rest)) => !text.is_empty() && glob(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && glob(rest, &text[1..]),
    }
}

fn hashed_matches(salt: &[u8], hash: &[u8; 20], name: &str) -> bool {
    let Ok(mut mac) = <Hmac<Sha1> as KeyInit>::new_from_slice(salt) else {
        return false;
    };
    mac.update(name.as_bytes());
    mac.verify_slice(hash).is_ok()
}

/// Does an entry's host field apply to `name` (from `host_name`)? A list
/// applies when some pattern matches and no negated pattern does.
pub fn host_matches(patterns: &HostPatterns, name: &str) -> bool {
    match patterns {
        HostPatterns::HashedName { salt, hash } => hashed_matches(salt, hash, name),
        HostPatterns::Patterns(list) => {
            let text: Vec<char> = name.chars().collect();
            let mut matched = false;
            for p in list {
                let (negated, p) = match p.strip_prefix('!') {
                    Some(rest) => (true, rest),
                    None => (false, p.as_str()),
                };
                let p: Vec<char> = p.to_ascii_lowercase().chars().collect();
                if glob(&p, &text) {
                    if negated {
                        return false;
                    }
                    matched = true;
                }
            }
            matched
        }
    }
}

/// What one file says about `key` for `name`.
#[derive(Debug, Default, PartialEq, Eq)]
struct FileVerdict {
    known: bool,
    changed: Option<usize>,
    revoked: Option<usize>,
}

fn scan(text: &str, name: &str, key: &PublicKey) -> FileVerdict {
    let mut v = FileVerdict::default();
    for (i, line) in text.lines().enumerate() {
        // One line at a time, so a bad line only skips itself.
        let Some(Ok(entry)) = KnownHosts::new(line).next() else {
            continue;
        };
        if !host_matches(entry.host_patterns(), name) {
            continue;
        }
        let same_key = entry.public_key().key_data() == key.key_data();
        match entry.marker() {
            Some(Marker::Revoked) => {
                if same_key {
                    v.revoked.get_or_insert(i + 1);
                }
            }
            Some(Marker::CertAuthority) => {}
            None => {
                if same_key {
                    v.known = true;
                } else if entry.public_key().algorithm() == key.algorithm() {
                    v.changed.get_or_insert(i + 1);
                }
            }
        }
    }
    v
}

/// Check `key` for `host:port` in every file. A revoked or changed key in
/// any file wins over a match elsewhere. Missing files are skipped; a file
/// that exists but cannot be read is refused.
pub fn check(files: &[PathBuf], host: &str, port: u16, key: &PublicKey) -> HostKeyStatus {
    let name = host_name(host, port);
    let mut known = false;
    let mut changed = None;
    for file in files.iter().filter(|f| f.exists()) {
        let bytes = match std::fs::read(file) {
            Ok(b) => b,
            Err(e) => {
                return HostKeyStatus::Unreadable {
                    file: file.clone(),
                    error: e.to_string(),
                };
            }
        };
        let v = scan(&String::from_utf8_lossy(&bytes), &name, key);
        if let Some(line) = v.revoked {
            return HostKeyStatus::Revoked {
                file: file.clone(),
                line,
            };
        }
        if changed.is_none()
            && let Some(line) = v.changed
        {
            changed = Some((file.clone(), line));
        }
        known |= v.known;
    }
    match (changed, known) {
        (Some((file, line)), _) => HostKeyStatus::Changed { file, line },
        (None, true) => HostKeyStatus::Known,
        (None, false) => HostKeyStatus::Unknown,
    }
}

/// Append `key` for `host:port` to `file` (created with parents if
/// missing), unless the file already records exactly this key for the
/// host. The line is `<host_name> <type> <base64>`.
pub fn learn(file: &Path, host: &str, port: u16, key: &PublicKey) -> Result<(), String> {
    let err = |e: io::Error| format!("{}: {e}", file.display());
    let existing = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(err(e)),
    };
    let name = host_name(host, port);
    if scan(&String::from_utf8_lossy(&existing), &name, key).known {
        return Ok(());
    }
    let mut bare = key.clone();
    bare.set_comment("");
    let encoded = bare
        .to_openssh()
        .map_err(|e| format!("{}: {e}", file.display()))?;
    let mut line = String::new();
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        line.push('\n');
    }
    line.push_str(&format!("{name} {}\n", encoded.trim_end()));
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .map_err(err)
}
