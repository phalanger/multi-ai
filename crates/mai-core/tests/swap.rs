//! Swapping a new probe binary into place on the local file system.

use std::path::Path;

use mai_core::swap::{LocalFiles, swap_in};

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn dir_str(dir: &Path) -> String {
    dir.to_string_lossy().into_owned()
}

#[tokio::test]
async fn first_install_renames_the_upload() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mai-probe.upload"), "v1").unwrap();
    swap_in(&LocalFiles, &dir_str(dir.path()), "mai-probe", 1)
        .await
        .unwrap();
    assert_eq!(names(dir.path()), vec!["mai-probe"]);
}

#[tokio::test]
async fn replacing_leaves_only_the_new_binary() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mai-probe"), "v1").unwrap();
    std::fs::write(dir.path().join("mai-probe.upload"), "v2").unwrap();
    // Leftovers of earlier swaps are cleaned up.
    std::fs::write(dir.path().join("mai-probe.old"), "v0").unwrap();
    std::fs::write(dir.path().join("mai-probe.old-17"), "v0").unwrap();
    swap_in(&LocalFiles, &dir_str(dir.path()), "mai-probe", 2)
        .await
        .unwrap();
    assert_eq!(names(dir.path()), vec!["mai-probe"]);
    assert_eq!(std::fs::read(dir.path().join("mai-probe")).unwrap(), b"v2");
}

#[tokio::test]
async fn missing_upload_keeps_the_old_binary() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("mai-probe"), "v1").unwrap();
    let r = swap_in(&LocalFiles, &dir_str(dir.path()), "mai-probe", 3).await;
    assert!(r.is_err());
    assert_eq!(std::fs::read(dir.path().join("mai-probe")).unwrap(), b"v1");
}

/// Windows refuses to delete a running executable but lets it be renamed,
/// which is what the swap relies on.
#[cfg(windows)]
#[tokio::test]
async fn running_executable_is_replaced_on_windows() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("mai-probe.exe");
    std::fs::copy(r"C:\Windows\System32\ping.exe", &exe).unwrap();
    let mut running = std::process::Command::new(&exe)
        .args(["-n", "30", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        std::fs::remove_file(&exe).is_err(),
        "a running exe is locked"
    );

    std::fs::write(dir.path().join("mai-probe.exe.upload"), "new").unwrap();
    swap_in(&LocalFiles, &dir_str(dir.path()), "mai-probe.exe", 4)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), b"new");
    // The old binary is still locked, so it stays aside until next time.
    assert_eq!(
        names(dir.path()),
        vec!["mai-probe.exe", "mai-probe.exe.old"]
    );

    running.kill().unwrap();
    running.wait().unwrap();
    std::fs::write(dir.path().join("mai-probe.exe.upload"), "newer").unwrap();
    swap_in(&LocalFiles, &dir_str(dir.path()), "mai-probe.exe", 5)
        .await
        .unwrap();
    assert_eq!(names(dir.path()), vec!["mai-probe.exe"]);
}
