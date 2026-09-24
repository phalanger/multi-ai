use mai_core::ssh::hostkey::{HostKeyStatus, check, fingerprint, learn};
use russh::keys::{Algorithm, PrivateKey};
use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;

fn key() -> PrivateKey {
    PrivateKey::random(&mut UnwrapErr(SysRng), Algorithm::Ed25519).unwrap()
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
        matches!(&status, HostKeyStatus::Changed { file: f, .. } if *f == file),
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
fn fingerprint_is_openssh_sha256_form() {
    let fp = fingerprint(key().public_key());
    assert!(fp.starts_with("SHA256:"), "{fp}");
    assert_eq!(fp.len(), "SHA256:".len() + 43, "{fp}");
}
