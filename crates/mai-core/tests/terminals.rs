//! Finding zellij for terminals when no path is known (B30) and the
//! locale of local terminals (B32).

use std::collections::HashMap;

use mai_core::terminals::{
    find_zellij_command, find_zellij_script, found_zellij, local_env_with, FALLBACK_LOCALE,
    ZELLIJ_DIRS,
};

#[test]
fn search_script_covers_path_dirs_and_login_shell() {
    let script = find_zellij_script();
    assert!(script.starts_with("command -v zellij || "), "{script}");
    assert!(
        script
            .contains("/opt/homebrew/bin /usr/local/bin \"$HOME/.cargo/bin\" \"$HOME/.local/bin\""),
        "{script}"
    );
    assert!(script.ends_with("-lc \"command -v zellij\""), "{script}");
    assert!(
        !script.contains('\''),
        "the script is wrapped in single quotes"
    );
    assert!(!script.contains('!'), "csh history expansion");
    assert_eq!(find_zellij_command(), format!("sh -c '{script}'"));
    assert_eq!(ZELLIJ_DIRS.len(), 4);
}

#[test]
fn found_path_is_the_last_absolute_line() {
    assert_eq!(
        found_zellij("/opt/homebrew/bin/zellij\n").as_deref(),
        Some("/opt/homebrew/bin/zellij")
    );
    // A login shell's startup files may print first.
    assert_eq!(
        found_zellij("Welcome!\nlast login: today\n/home/u/.cargo/bin/zellij\r\n").as_deref(),
        Some("/home/u/.cargo/bin/zellij")
    );
    assert_eq!(found_zellij(""), None);
    assert_eq!(found_zellij("zellij not found\n"), None);
}

#[cfg(unix)]
fn executable(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Run the search script the way the host would (`sh -c`), with `path`
/// as PATH and `home` as HOME.
#[cfg(unix)]
fn search(path: &str, home: &std::path::Path) -> String {
    let out = std::process::Command::new("/bin/sh")
        .args(["-c", &find_zellij_script()])
        .env_clear()
        .env("PATH", path)
        .env("HOME", home)
        .env("SHELL", "/bin/false")
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

#[cfg(unix)]
#[test]
fn search_finds_zellij_on_path_first() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    executable(&bin.join("zellij"));
    let out = search(&bin.to_string_lossy(), dir.path());
    assert_eq!(
        found_zellij(&out),
        Some(bin.join("zellij").to_string_lossy().into_owned())
    );
}

#[cfg(unix)]
#[test]
fn search_falls_back_to_home_dirs() {
    if ["/opt/homebrew/bin/zellij", "/usr/local/bin/zellij"]
        .iter()
        .any(|p| std::path::Path::new(p).exists())
    {
        return; // the machine's own zellij would be found first
    }
    let dir = tempfile::tempdir().unwrap();
    let z = dir.path().join(".local").join("bin").join("zellij");
    executable(&z);
    let out = search("/nonexistent", dir.path());
    assert_eq!(found_zellij(&out), Some(z.to_string_lossy().into_owned()));
}

#[cfg(unix)]
#[test]
fn local_zellij_falls_back_to_home_dirs() {
    use mai_core::terminals::local_zellij;
    if ["/opt/homebrew/bin/zellij", "/usr/local/bin/zellij"]
        .iter()
        .any(|p| std::path::Path::new(p).exists())
    {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let z = dir.path().join(".cargo").join("bin").join("zellij");
    executable(&z);
    let empty = std::ffi::OsString::from("/nonexistent");
    assert_eq!(
        local_zellij(Some(&empty), Some(dir.path())),
        z.into_os_string()
    );
    assert_eq!(local_zellij(Some(&empty), None), "zellij");
}

fn env_of(vars: &[(&str, &str)]) -> Vec<(&'static str, &'static str)> {
    let map: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    local_env_with(|k| map.get(k).cloned())
}

#[test]
fn local_terminal_keeps_the_users_utf8_locale() {
    let term_only = vec![("TERM", "xterm-256color")];
    let fallback = vec![
        ("TERM", "xterm-256color"),
        ("LANG", FALLBACK_LOCALE),
        ("LC_CTYPE", FALLBACK_LOCALE),
    ];
    if cfg!(windows) {
        assert_eq!(env_of(&[]), term_only);
        return;
    }
    assert_eq!(env_of(&[("LANG", "zh_CN.UTF-8")]), term_only);
    assert_eq!(env_of(&[("LC_CTYPE", "ja_JP.utf8")]), term_only);
    assert_eq!(env_of(&[]), fallback, "a GUI app may have no locale");
    assert_eq!(env_of(&[("LANG", "C")]), fallback);
    // LC_ALL wins over LANG, and an inherited LC_ALL must be overridden too.
    let mut with_lc_all = fallback.clone();
    with_lc_all.push(("LC_ALL", FALLBACK_LOCALE));
    assert_eq!(
        env_of(&[("LC_ALL", "C"), ("LANG", "en_US.UTF-8")]),
        with_lc_all
    );
    assert_eq!(
        env_of(&[("LC_ALL", ""), ("LANG", "de_DE.UTF-8")]),
        term_only
    );
}
