//! Resolve a host alias or `[user@]host[:port]` into connection
//! parameters, using OpenSSH config text (`~/.ssh/config`).
//!
//! `Match` blocks are not supported: the parser would attribute their
//! directives to the preceding `Host`, so they are removed before parsing
//! and reported by `config_warnings`.

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
    /// Jump hosts, outermost first (`ProxyJump`). Their own `ProxyJump`
    /// settings are not followed, so their `jumps` are always empty.
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
const DEFAULT_KEYS: &[&str] = &["id_ed25519", "id_ecdsa", "id_rsa"];

/// First word of a config line, lowercased (`Host`, `Match`, ...).
fn keyword(line: &str) -> String {
    line.trim_start()
        .split(|c: char| c.is_whitespace() || c == '=')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Lines of `text` without `Match` blocks. A block runs from a `Match`
/// line to the next `Host` or `Match` line.
fn without_match_blocks(text: &str) -> String {
    let mut out = String::new();
    let mut in_match = false;
    for line in text.lines() {
        match keyword(line).as_str() {
            "match" => in_match = true,
            "host" => in_match = false,
            _ => {}
        }
        if !in_match {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Things in `text` the user should know about, with 1-based line
/// numbers: one entry per `Match` block (ignored) and per `Include` (the
/// parser follows it, but cannot see `Match` blocks in the included file).
pub fn config_warnings(text: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| match keyword(l).as_str() {
            "match" => Some(format!(
                "ssh config line {}: Match blocks are not supported; their settings are ignored",
                i + 1
            )),
            "include" => Some(format!(
                "ssh config line {}: Include is followed, but Match blocks in included files are not detected; their settings may apply to the preceding Host",
                i + 1
            )),
            _ => None,
        })
        .collect()
}

/// Parse OpenSSH config text; unknown directives and `Match` blocks are
/// ignored.
pub fn parse_config(text: &str) -> Result<SshConfig, ConfigError> {
    SshConfig::default()
        .parse(
            &mut BufReader::new(without_match_blocks(text).as_bytes()),
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
    let mut spec = resolve_hop(config, target, default_user, home)?;
    spec.jumps = jumps_of(config, target)?
        .iter()
        .map(|j| resolve_hop(config, j, default_user, home))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(spec)
}

/// The `ProxyJump` hosts configured for `target`, outermost first.
fn jumps_of(config: &SshConfig, target: &str) -> Result<Vec<String>, ConfigError> {
    let params = config.query(split_target(target)?.1);
    Ok(params
        .proxy_jump
        .unwrap_or_default()
        .iter()
        .flat_map(|j| j.split(','))
        .map(str::trim)
        .filter(|j| !j.is_empty() && !j.eq_ignore_ascii_case("none"))
        .map(str::to_owned)
        .collect())
}

/// One note per jump host of `spec` that has a `ProxyJump` of its own:
/// it is not followed (design 3.1), which would otherwise only show as
/// failing connects (B35).
pub fn jump_warnings(config: &SshConfig, spec: &HostSpec) -> Vec<String> {
    spec.jumps
        .iter()
        .filter_map(|j| {
            let own = jumps_of(config, &j.alias).ok()?;
            (!own.is_empty()).then(|| {
                format!(
                    "ssh config: ProxyJump {} of jump host {} is not followed",
                    own.join(","),
                    j.alias
                )
            })
        })
        .collect()
}

/// One host's parameters, without jump hosts.
fn resolve_hop(
    config: &SshConfig,
    target: &str,
    default_user: &str,
    home: &Path,
) -> Result<HostSpec, ConfigError> {
    let (user, name, port) = split_target(target)?;
    let params = config.query(name);
    let identity_files = params.identity_file.clone().unwrap_or_else(|| {
        DEFAULT_KEYS
            .iter()
            .map(|k| home.join(".ssh").join(k))
            .collect()
    });
    Ok(HostSpec {
        alias: target.to_owned(),
        host: params.host_name.clone().unwrap_or_else(|| name.to_owned()),
        port: port.or(params.port).unwrap_or(DEFAULT_PORT),
        user: user
            .map(str::to_owned)
            .or(params.user.clone())
            .unwrap_or_else(|| default_user.to_owned()),
        identity_files,
        jumps: Vec::new(),
    })
}
