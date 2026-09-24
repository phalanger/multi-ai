//! Resolve a host alias or `[user@]host[:port]` into connection
//! parameters, using OpenSSH config text (`~/.ssh/config`).

use std::fmt;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use ssh2_config::{ParseRule, SshConfig};

/// Everything needed to open one SSH connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    /// What the user typed (alias or `[user@]host[:port]`).
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Tried in order; missing files are skipped at auth time.
    pub identity_files: Vec<PathBuf>,
    /// Jump hosts, outermost first (`ProxyJump`).
    pub jumps: Vec<HostSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

const DEFAULT_PORT: u16 = 22;
const MAX_JUMP_DEPTH: usize = 8;
const DEFAULT_KEYS: &[&str] = &["id_ed25519", "id_ecdsa", "id_rsa"];

/// Parse OpenSSH config text; unknown directives are ignored.
pub fn parse_config(text: &str) -> Result<SshConfig, ConfigError> {
    SshConfig::default()
        .parse(
            &mut BufReader::new(text.as_bytes()),
            ParseRule::ALLOW_UNKNOWN_FIELDS,
        )
        .map_err(|e| ConfigError(format!("ssh config: {e}")))
}

/// Split `[user@]host[:port]`. A bare IPv6 address (several `:`) is kept
/// whole unless written as `[addr]:port`.
fn split_target(target: &str) -> Result<(Option<&str>, &str, Option<u16>), ConfigError> {
    let (user, rest) = match target.rsplit_once('@') {
        Some((u, r)) => (Some(u), r),
        None => (None, target),
    };
    let bad_port = || ConfigError(format!("bad port in '{target}'"));
    if let Some(inner) = rest.strip_prefix('[') {
        let (addr, tail) = inner
            .split_once(']')
            .ok_or_else(|| ConfigError(format!("unclosed '[' in '{target}'")))?;
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(p.parse().map_err(|_| bad_port())?),
            None => None,
        };
        return Ok((user, addr, port));
    }
    match rest.split_once(':') {
        Some((host, port)) if !port.contains(':') => {
            Ok((user, host, Some(port.parse().map_err(|_| bad_port())?)))
        }
        _ => Ok((user, rest, None)),
    }
}

/// Resolve `target` against `config`. `default_user` applies when neither
/// the target nor the config names a user; `home` locates default keys.
pub fn resolve(
    config: &SshConfig,
    target: &str,
    default_user: &str,
    home: &Path,
) -> Result<HostSpec, ConfigError> {
    resolve_depth(config, target, default_user, home, 0)
}

fn resolve_depth(
    config: &SshConfig,
    target: &str,
    default_user: &str,
    home: &Path,
    depth: usize,
) -> Result<HostSpec, ConfigError> {
    if depth > MAX_JUMP_DEPTH {
        return Err(ConfigError(format!(
            "ProxyJump chain deeper than {MAX_JUMP_DEPTH} at '{target}' (loop?)"
        )));
    }
    let (user, name, port) = split_target(target)?;
    let params = config.query(name);
    let identity_files = params.identity_file.clone().unwrap_or_else(|| {
        DEFAULT_KEYS
            .iter()
            .map(|k| home.join(".ssh").join(k))
            .collect()
    });
    let jumps = params
        .proxy_jump
        .clone()
        .unwrap_or_default()
        .iter()
        .flat_map(|j| j.split(','))
        .map(str::trim)
        .filter(|j| !j.is_empty() && !j.eq_ignore_ascii_case("none"))
        .map(|j| resolve_depth(config, j, default_user, home, depth + 1))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HostSpec {
        alias: target.to_owned(),
        host: params.host_name.clone().unwrap_or_else(|| name.to_owned()),
        port: port.or(params.port).unwrap_or(DEFAULT_PORT),
        user: user
            .map(str::to_owned)
            .or(params.user.clone())
            .unwrap_or_else(|| default_user.to_owned()),
        identity_files,
        jumps,
    })
}
