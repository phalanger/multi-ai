//! User interaction and secret storage used while connecting.
//!
//! The app implements `Prompter` with dialogs; tests use scripted ones.
//! Secrets (passwords, key passphrases) live only in a `SecretStore`,
//! by default the OS keychain.

use std::future::Future;
use std::path::Path;

/// A secret typed by the user, and whether to remember it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Secret {
    pub value: String,
    pub remember: bool,
}

/// One keyboard-interactive prompt: text and whether input is echoed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KbdPrompt {
    pub text: String,
    pub echo: bool,
}

/// Questions asked while connecting. `None` / `false` means the user
/// cancelled, which aborts that authentication step.
pub trait Prompter: Send + Sync + 'static {
    /// Unknown host key: trust and remember it?
    fn confirm_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> impl Future<Output = bool> + Send;

    fn password(&self, user: &str, host: &str) -> impl Future<Output = Option<Secret>> + Send;

    fn passphrase(&self, key_file: &Path) -> impl Future<Output = Option<Secret>> + Send;

    /// Answers for a keyboard-interactive round (one per prompt).
    fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[KbdPrompt],
    ) -> impl Future<Output = Option<Vec<String>>> + Send;
}

/// Where remembered secrets are kept.
pub trait SecretStore: Send + Sync + 'static {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// Store key for a host's login password (`<host_id>/password`).
pub fn password_key(host_id: &str) -> String {
    format!("{host_id}/password")
}

/// Store key for a private key's passphrase (`passphrase:<path>`).
pub fn passphrase_key(key_file: &Path) -> String {
    format!("passphrase:{}", key_file.display())
}

const KEYRING_SERVICE: &str = "multi-ai";

/// macOS Keychain / Windows Credential Manager / Secret Service.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringStore;

impl SecretStore for KeyringStore {
    fn get(&self, key: &str) -> Option<String> {
        keyring::Entry::new(KEYRING_SERVICE, key)
            .ok()?
            .get_password()
            .ok()
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(KEYRING_SERVICE, key)
            .and_then(|e| e.set_password(value))
            .map_err(|e| e.to_string())
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        keyring::Entry::new(KEYRING_SERVICE, key)
            .and_then(|e| e.delete_credential())
            .map_err(|e| e.to_string())
    }
}
