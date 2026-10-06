//! Public-key authentication from key files that asks for a passphrase
//! only when the server has accepted the public key (B9): russh first
//! offers the public key (`USERAUTH_REQUEST` without signature) and asks
//! the signer only after `USERAUTH_PK_OK`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use russh::keys::agent::AgentIdentity;
use russh::keys::ssh_key::encoding::Encode;
use russh::keys::ssh_key::private::KeypairData;
use russh::keys::ssh_key::public::KeyData;
use russh::keys::{Algorithm, HashAlg, PrivateKey, PublicKey};
use signature::Signer as _;

use super::auth::{Prompter, SecretStore, legacy_passphrase_key, passphrase_key};

/// The public half of a key file without decrypting it: `<file>.pub` if
/// present, else the public key stored in an OpenSSH private key file
/// (readable even when the private part is encrypted). `None` for formats
/// that need decrypting first (legacy PEM).
pub fn public_key_of(path: &Path) -> Option<PublicKey> {
    let mut pub_path = OsString::from(path.as_os_str());
    pub_path.push(".pub");
    if let Ok(k) = russh::keys::load_public_key(PathBuf::from(pub_path)) {
        return Some(k);
    }
    let text = std::fs::read_to_string(path).ok()?;
    let key = PrivateKey::from_openssh(text.trim()).ok()?;
    Some(key.public_key().clone())
}

/// Load a private key; ask for (and optionally remember) its passphrase.
/// Keychain failures are added to `notes`.
pub async fn load_key<P: Prompter, S: SecretStore>(
    path: &Path,
    prompter: &P,
    secrets: &S,
    notes: &mut Vec<String>,
) -> Option<PrivateKey> {
    match russh::keys::load_secret_key(path, None) {
        Ok(k) => return Some(k),
        Err(russh::keys::Error::KeyIsEncrypted) => {}
        Err(_) => return None,
    }
    let key = passphrase_key(path);
    if let Some(k) = migrate_passphrase(path, &key, secrets, notes) {
        return Some(k);
    }
    if let Some(pass) = secrets.get(&key) {
        if let Ok(k) = russh::keys::load_secret_key(path, Some(&pass)) {
            return Some(k);
        }
        if let Err(e) = secrets.delete(&key) {
            notes.push(format!(
                "could not remove the stale passphrase of {} from the keychain: {e}",
                path.display()
            ));
        }
    }
    let s = prompter.passphrase(path).await?;
    let k = russh::keys::load_secret_key(path, Some(&s.value)).ok()?;
    if s.remember
        && let Err(e) = secrets.set(&key, &s.value)
    {
        notes.push(format!(
            "could not save the passphrase of {} in the keychain: {e}",
            path.display()
        ));
    }
    Some(k)
}

/// A passphrase remembered under the old key (`legacy_passphrase_key`)
/// and nothing under the current one: decrypt with it, move it to the
/// current key and remove the old entry. Returns the key if that worked.
fn migrate_passphrase<S: SecretStore>(
    path: &Path,
    key: &str,
    secrets: &S,
    notes: &mut Vec<String>,
) -> Option<PrivateKey> {
    let legacy = legacy_passphrase_key(path);
    if legacy == key || secrets.get(key).is_some() {
        return None;
    }
    let pass = secrets.get(&legacy)?;
    let decrypted = russh::keys::load_secret_key(path, Some(&pass)).ok();
    if decrypted.is_some()
        && let Err(e) = secrets.set(key, &pass)
    {
        notes.push(format!(
            "could not save the passphrase of {} in the keychain: {e}",
            path.display()
        ));
    }
    if let Err(e) = secrets.delete(&legacy) {
        notes.push(format!(
            "could not remove the old passphrase entry of {} from the keychain: {e}",
            path.display()
        ));
    }
    decrypted
}

/// `to_sign` followed by the SSH signature of `to_sign` (an SSH string
/// holding the signature blob), as russh expects from a signer.
pub fn signed(key: &PrivateKey, hash_alg: Option<HashAlg>, to_sign: &[u8]) -> Option<Vec<u8>> {
    let sig = match key.key_data() {
        KeypairData::Rsa(rsa) => (rsa, hash_alg).try_sign(to_sign).ok()?,
        _ => key.try_sign(to_sign).ok()?,
    };
    let blob = sig.encode_vec().ok()?;
    let mut out = to_sign.to_vec();
    blob.encode(&mut out).ok()?;
    Some(out)
}

/// `to_sign` followed by a well-formed signature that cannot verify, for
/// when the private key is unavailable (passphrase refused). The server
/// then rejects the key normally and authentication moves on; russh offers
/// no way to withdraw a key once the server accepted it. RSA gets filler
/// bytes of the modulus size (generating an RSA key would be slow); other
/// types are signed with a throwaway key of the same type.
pub fn bogus_signed(public: &PublicKey, hash_alg: Option<HashAlg>, to_sign: &[u8]) -> Vec<u8> {
    let blob = match public.key_data() {
        KeyData::Rsa(rsa) => {
            let mut blob = Vec::new();
            let name = Algorithm::Rsa { hash: hash_alg };
            let filler = vec![0x5a_u8; rsa.n().as_positive_bytes().map_or(256, <[u8]>::len)];
            let _ = name.as_str().encode(&mut blob);
            let _ = filler.encode(&mut blob);
            Some(blob)
        }
        _ => PrivateKey::random(&mut russh::keys::key::safe_rng(), public.algorithm())
            .ok()
            .and_then(|k| match k.key_data() {
                KeypairData::Rsa(rsa) => (rsa, hash_alg).try_sign(to_sign).ok(),
                _ => k.try_sign(to_sign).ok(),
            })
            .and_then(|s| s.encode_vec().ok()),
    };
    let mut out = to_sign.to_vec();
    let _ = blob.unwrap_or_default().encode(&mut out);
    out
}

/// Error type the russh `Signer` trait requires: russh raises it when the
/// session it would send the request on has gone away.
#[derive(Debug)]
pub struct SignError;

impl std::fmt::Display for SignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the SSH session closed during public key authentication")
    }
}

impl std::error::Error for SignError {}

impl From<russh::SendError> for SignError {
    fn from(_: russh::SendError) -> Self {
        SignError
    }
}

/// Signs with a key file, decrypting it only when asked to sign.
pub struct FileSigner<'a, P, S> {
    pub path: &'a Path,
    pub prompter: &'a P,
    pub secrets: &'a S,
    pub notes: &'a mut Vec<String>,
    /// Set by `auth_sign` when no real signature could be produced (the
    /// passphrase was refused or the key could not be loaded) and a bogus
    /// one was sent instead. Callers then must not count the key as tried.
    pub declined: bool,
}

impl<P: Prompter, S: SecretStore> russh::Signer for FileSigner<'_, P, S> {
    type Error = SignError;

    async fn auth_sign(
        &mut self,
        key: &AgentIdentity,
        hash_alg: Option<HashAlg>,
        to_sign: Vec<u8>,
    ) -> Result<Vec<u8>, SignError> {
        let private = load_key(self.path, self.prompter, self.secrets, self.notes).await;
        if let Some(out) = private.and_then(|k| signed(&k, hash_alg, &to_sign)) {
            return Ok(out);
        }
        self.declined = true;
        let AgentIdentity::PublicKey { key: public, .. } = key else {
            return Ok(to_sign);
        };
        Ok(bogus_signed(public, hash_alg, &to_sign))
    }
}
