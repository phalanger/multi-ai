//! Pure parts of the real connector: error classification, local
//! platform detection and binary placement.

use std::cell::Cell;
use std::path::PathBuf;

use mai_core::connect::{deploy_open_error, local_remote, place_binary, ssh_open_error};
use mai_core::deploy::DeployError;
use mai_core::host::{OpenError, Problem};
use mai_core::ssh::client::SshError;

#[test]
fn transport_errors_retry_and_credentials_need_the_user() {
    assert_eq!(
        ssh_open_error(SshError::Connect("refused".into())),
        OpenError::Retry("refused".into())
    );
    assert_eq!(
        ssh_open_error(SshError::Channel("eof".into())),
        OpenError::Retry("eof".into())
    );
    assert_eq!(
        ssh_open_error(SshError::Auth("no method".into())),
        OpenError::NeedsUser(Problem::Auth("no method".into()))
    );
    assert_eq!(
        ssh_open_error(SshError::HostKeyRejected {
            host: "h".into(),
            port: 22,
            fingerprint: "SHA256:x".into()
        }),
        OpenError::NeedsUser(Problem::HostKeyRejected)
    );
    let changed = ssh_open_error(SshError::HostKeyChanged {
        host: "h".into(),
        port: 22,
        file: PathBuf::from("kh"),
        line: 3,
    });
    assert_eq!(
        changed,
        OpenError::NeedsUser(Problem::HostKeyChanged {
            file: PathBuf::from("kh"),
            line: 3
        })
    );
}

#[test]
fn deploy_errors_need_the_user_unless_ssh_dropped() {
    assert_eq!(
        deploy_open_error(DeployError::Ssh(SshError::Connect("reset".into()))),
        OpenError::Retry("reset".into())
    );
    match deploy_open_error(DeployError::Upload("locked".into())) {
        OpenError::NeedsUser(Problem::Deploy(m)) => assert!(m.contains("locked"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn local_platform_has_a_probe_target() {
    let home = std::env::temp_dir();
    let remote = local_remote(&home).expect("supported platform");
    let target = remote.target().expect("probe target");
    assert!(target.contains(std::env::consts::ARCH), "{target}");
    let path = remote.probe_path(".mai");
    assert!(path.starts_with(&*home.to_string_lossy()), "{path}");
    assert!(path.contains("mai-probe"), "{path}");
}

#[test]
fn binary_is_placed_only_when_it_differs() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join(".mai").join("bin").join("mai-probe");
    let stops = Cell::new(0);
    let stop = || stops.set(stops.get() + 1);

    assert!(place_binary(&exe, b"v1", stop).unwrap());
    assert_eq!(std::fs::read(&exe).unwrap(), b"v1");
    assert_eq!(stops.get(), 0, "nothing to stop on first install");

    assert!(!place_binary(&exe, b"v1", stop).unwrap());
    assert_eq!(stops.get(), 0);

    assert!(place_binary(&exe, b"v2", stop).unwrap());
    assert_eq!(std::fs::read(&exe).unwrap(), b"v2");
    assert_eq!(stops.get(), 1, "old probe stopped before replacing it");
    let leftovers: Vec<_> = std::fs::read_dir(exe.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(leftovers.len(), 1, "{leftovers:?}");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}
