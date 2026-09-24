//! Shared helpers for the manual-check examples.
//!
//! Environment:
//! - `MAI_SSH_CONFIG`: ssh config file (default `~/.ssh/config`)
//! - `MAI_LEARN_TO`: where confirmed host keys go (default: a temp file);
//!   it is also checked
//! - `MAI_SKIP_USER_KNOWN_HOSTS=1`: ignore `~/.ssh/known_hosts`
//!
//! Prompts are read from the terminal; passwords, passphrases and
//! non-echo keyboard-interactive answers are read without echo.
//! Secrets are kept in memory, never in the OS keychain.

use std::collections::HashMap;
use std::future::Future;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mai_core::ssh::auth::{KbdPrompt, Prompter, Secret, SecretStore};
use mai_core::ssh::client::{ConnectOptions, SshSession, connect};
use mai_core::ssh::config::{HostSpec, parse_config, resolve};

pub struct TermPrompter;

fn ask(question: &str) -> Option<String> {
    print!("{question}");
    std::io::stdout().flush().ok()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok()?;
    let line = line.trim_end().to_owned();
    (!line.is_empty()).then_some(line)
}

fn ask_hidden(question: &str) -> Option<String> {
    let line = rpassword::prompt_password(question).ok()?;
    (!line.is_empty()).then_some(line)
}

impl Prompter for TermPrompter {
    fn confirm_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> impl Future<Output = bool> + Send {
        let q = format!("Unknown host key for {host}:{port}\n  {fingerprint}\nTrust it? [y/N] ");
        async move { ask(&q).is_some_and(|a| a.eq_ignore_ascii_case("y")) }
    }

    fn password(&self, user: &str, host: &str) -> impl Future<Output = Option<Secret>> + Send {
        let q = format!("Password for {user}@{host}: ");
        async move {
            ask_hidden(&q).map(|value| Secret {
                value,
                remember: false,
            })
        }
    }

    fn passphrase(&self, key_file: &Path) -> impl Future<Output = Option<Secret>> + Send {
        let q = format!("Passphrase for {}: ", key_file.display());
        async move {
            ask_hidden(&q).map(|value| Secret {
                value,
                remember: false,
            })
        }
    }

    fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[KbdPrompt],
    ) -> impl Future<Output = Option<Vec<String>>> + Send {
        let header = format!("{name} {instructions}");
        let asks: Vec<KbdPrompt> = prompts.to_vec();
        async move {
            println!("{header}");
            asks.iter()
                .map(|p| {
                    if p.echo {
                        ask(&p.text)
                    } else {
                        ask_hidden(&p.text)
                    }
                })
                .collect()
        }
    }
}

#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().ok()?.get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|e| e.to_string())?
            .insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.remove(key);
        Ok(())
    }
}

pub fn home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map_or_else(|| PathBuf::from("."), PathBuf::from)
}

pub fn spec(target: &str) -> HostSpec {
    let home = home();
    let file = std::env::var_os("MAI_SSH_CONFIG")
        .map_or_else(|| home.join(".ssh").join("config"), PathBuf::from);
    let cfg = parse_config(&std::fs::read_to_string(file).unwrap_or_default()).expect("ssh config");
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    resolve(&cfg, target, &user, &home).expect("resolve target")
}

pub fn options() -> ConnectOptions {
    let learn_to = std::env::var_os("MAI_LEARN_TO").map_or_else(
        || std::env::temp_dir().join("mai-example-known_hosts"),
        PathBuf::from,
    );
    let mut known_hosts = vec![learn_to.clone()];
    if std::env::var_os("MAI_SKIP_USER_KNOWN_HOSTS").is_none() {
        known_hosts.insert(0, home().join(".ssh").join("known_hosts"));
    }
    ConnectOptions {
        known_hosts,
        learn_to,
        timeout: Duration::from_secs(20),
    }
}

pub async fn connect_or_exit(target: &str) -> SshSession<TermPrompter> {
    let spec = spec(target);
    println!(
        "connecting: {}@{}:{} (jumps: {})",
        spec.user,
        spec.host,
        spec.port,
        spec.jumps.len()
    );
    match connect(
        &spec,
        &options(),
        Arc::new(TermPrompter),
        &MemoryStore::default(),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
