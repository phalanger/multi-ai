use std::path::{Path, PathBuf};

use hmac::{Hmac, KeyInit, Mac};
use mai_core::ssh::hostkey::{HostKeyStatus, check, fingerprint, host_name, learn};
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use sha1::Sha1;
use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;

fn key() -> PrivateKey {
    PrivateKey::random(&mut UnwrapErr(SysRng), Algorithm::Ed25519).unwrap()
}

/// `<type> <base64>` of a public key, as in a known_hosts line.
fn openssh(k: &PublicKey) -> String {
    let mut k = k.clone();
    k.set_comment("");
    k.to_openssh().unwrap()
}

fn write(dir: &Path, text: &str) -> Vec<PathBuf> {
    let file = dir.join("known_hosts");
    std::fs::write(&file, text).unwrap();
    vec![file]
}

#[test]
fn learned_key_is_known_on_that_host_and_port_only() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("sub").join("known_hosts");
    let k = key();
    learn(&file, "example.org", 2222, k.public_key()).unwrap();
    let files = vec![file];
    assert_eq!(
        check(&files, "example.org", 2222, k.public_key()),
        HostKeyStatus::Known
    );
    assert_eq!(
        check(&files, "example.org", 22, k.public_key()),
        HostKeyStatus::Unknown
    );
    assert_eq!(
        check(&files, "other.org", 2222, k.public_key()),
        HostKeyStatus::Unknown
    );
}

#[test]
fn different_key_of_same_algorithm_is_changed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("known_hosts");
    learn(&file, "h", 22, key().public_key()).unwrap();
    let status = check(std::slice::from_ref(&file), "h", 22, key().public_key());
    assert!(
        matches!(&status, HostKeyStatus::Changed { file: f, line: 1 } if *f == file),
        "{status:?}"
    );
}

#[test]
fn changed_in_any_file_wins_over_known_elsewhere() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good");
    let bad = dir.path().join("bad");
    let k = key();
    learn(&good, "h", 22, k.public_key()).unwrap();
    learn(&bad, "h", 22, key().public_key()).unwrap();
    let status = check(&[good, bad], "h", 22, k.public_key());
    assert!(
        matches!(status, HostKeyStatus::Changed { .. }),
        "{status:?}"
    );
}

#[test]
fn missing_files_are_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let files = vec![dir.path().join("nope")];
    assert_eq!(
        check(&files, "h", 22, key().public_key()),
        HostKeyStatus::Unknown
    );
}

#[test]
fn unreadable_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    // A directory exists but cannot be read as a file.
    let files = vec![dir.path().to_path_buf()];
    let status = check(&files, "h", 22, key().public_key());
    assert!(
        matches!(status, HostKeyStatus::Unreadable { .. }),
        "{status:?}"
    );
}

#[test]
fn fingerprint_is_openssh_sha256_form() {
    let fp = fingerprint(key().public_key());
    assert!(fp.starts_with("SHA256:"), "{fp}");
    assert_eq!(fp.len(), "SHA256:".len() + 43, "{fp}");
}

#[test]
fn bad_lines_are_skipped_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let k = key();
    let mut text: Vec<u8> = b"garbage line here\n".to_vec();
    text.extend_from_slice(&[b'h', b' ', 0xff, 0xfe, b'\n']);
    text.extend_from_slice(b"h ssh-ed25519 not-base64!\n");
    text.extend_from_slice(format!("# comment\nh {}\r\n", openssh(k.public_key())).as_bytes());
    let file = dir.path().join("known_hosts");
    std::fs::write(&file, text).unwrap();
    assert_eq!(
        check(&[file], "h", 22, k.public_key()),
        HostKeyStatus::Known
    );
}

#[test]
fn mixed_case_host_matches_lowercase_entry() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("known_hosts");
    let k = key();
    learn(&file, "Example.ORG", 22, k.public_key()).unwrap();
    let files = vec![file];
    assert_eq!(
        check(&files, "eXaMpLe.org", 22, k.public_key()),
        HostKeyStatus::Known
    );
}

#[test]
fn hashed_host_names_match() {
    let dir = tempfile::tempdir().unwrap();
    let k = key();
    let salt = b"0123456789abcdefghij";
    let mut mac = <Hmac<Sha1> as KeyInit>::new_from_slice(salt).unwrap();
    mac.update(host_name("db.example.org", 2200).as_bytes());
    let hash = mac.finalize().into_bytes();
    use russh::keys::ssh_key::encoding::base64::{Base64, Encoding};
    let line = format!(
        "|1|{}|{} {}\n",
        Base64::encode_string(salt),
        Base64::encode_string(&hash),
        openssh(k.public_key())
    );
    let files = write(dir.path(), &line);
    assert_eq!(
        check(&files, "DB.example.org", 2200, k.public_key()),
        HostKeyStatus::Known
    );
    assert_eq!(
        check(&files, "db.example.org", 22, k.public_key()),
        HostKeyStatus::Unknown
    );
}

#[test]
fn globs_and_negation() {
    let dir = tempfile::tempdir().unwrap();
    let k = key();
    let line = format!(
        "*.example.org,!bad.example.org,[10.0.0.?]:2222 {}\n",
        openssh(k.public_key())
    );
    let files = write(dir.path(), &line);
    let status = |h: &str, p: u16| check(&files, h, p, k.public_key());
    assert_eq!(status("good.example.org", 22), HostKeyStatus::Known);
    assert_eq!(status("bad.example.org", 22), HostKeyStatus::Unknown);
    assert_eq!(status("example.org", 22), HostKeyStatus::Unknown);
    assert_eq!(status("10.0.0.7", 2222), HostKeyStatus::Known);
    assert_eq!(status("10.0.0.7", 22), HostKeyStatus::Unknown);
}

#[test]
fn revoked_key_is_refused_even_if_known() {
    let dir = tempfile::tempdir().unwrap();
    let k = key();
    let text = format!("h {key}\n@revoked * {key}\n", key = openssh(k.public_key()));
    let files = write(dir.path(), &text);
    let status = check(&files, "h", 22, k.public_key());
    assert!(
        matches!(status, HostKeyStatus::Revoked { line: 2, .. }),
        "{status:?}"
    );
    // Revocation is per key: another key for the host is unaffected.
    let other = key();
    assert!(matches!(
        check(&files, "h", 22, other.public_key()),
        HostKeyStatus::Changed { .. }
    ));
}

#[test]
fn cert_authority_lines_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let ca = key();
    let text = format!("@cert-authority * {}\n", openssh(ca.public_key()));
    let files = write(dir.path(), &text);
    assert_eq!(
        check(&files, "h", 22, ca.public_key()),
        HostKeyStatus::Unknown
    );
}

#[test]
fn learning_twice_writes_one_line() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("known_hosts");
    // An existing file without a trailing newline.
    let other = key();
    std::fs::write(&file, format!("x {}", openssh(other.public_key()))).unwrap();
    let k = key();
    learn(&file, "h", 22, k.public_key()).unwrap();
    learn(&file, "h", 22, k.public_key()).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2, "{text:?}");
    assert_eq!(lines[1], format!("h {}", openssh(k.public_key())));
    assert!(text.ends_with('\n'));
}
