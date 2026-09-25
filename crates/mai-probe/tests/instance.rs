use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use mai_probe::instance::{data_dir, forget_self, pid_file, record_self, stop_recorded};

#[test]
fn deploy_dir_wins_over_mai_home() {
    let exe = PathBuf::from("/home/u/.mai/bin/mai-probe");
    let got = data_dir(
        Some(&exe),
        Some(OsStr::new("/elsewhere")),
        Path::new("/home/u"),
    );
    assert_eq!(got, PathBuf::from("/home/u/.mai"));
    let e2e = PathBuf::from("/home/u/.mai-e2e/bin/mai-probe");
    assert_eq!(
        data_dir(Some(&e2e), None, Path::new("/home/u")),
        PathBuf::from("/home/u/.mai-e2e")
    );
}

#[test]
fn other_locations_use_mai_home_then_default() {
    let exe = PathBuf::from("/usr/local/bin/mai-probe");
    let home = Path::new("/home/u");
    assert_eq!(
        data_dir(Some(&exe), Some(OsStr::new("/data")), home),
        PathBuf::from("/data")
    );
    assert_eq!(data_dir(Some(&exe), None, home), home.join(".mai"));
    assert_eq!(data_dir(None, None, home), home.join(".mai"));
}

#[test]
fn pid_file_names_are_per_client() {
    let d = Path::new("d");
    assert_eq!(pid_file(d, "mac-1"), d.join("serve-mac-1.pid"));
    assert_eq!(pid_file(d, ""), d.join("serve.pid"));
    assert_eq!(pid_file(d, "../x"), d.join("serve-x.pid"));
}

#[test]
fn own_pid_is_recorded_never_stopped_and_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let file = pid_file(dir.path(), "me");
    record_self(&file).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        std::process::id().to_string()
    );
    assert_eq!(stop_recorded(&file, Duration::from_millis(10)), None);
    forget_self(&file);
    assert!(!file.exists());
}

#[test]
fn stale_or_foreign_pid_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let file = pid_file(dir.path(), "x");
    // Not a mai-probe process (this test binary has another name).
    std::fs::write(&file, u32::MAX.to_string()).unwrap();
    assert_eq!(stop_recorded(&file, Duration::from_millis(10)), None);
    forget_self(&file);
    assert!(file.exists(), "a pid file naming another process is kept");
}
