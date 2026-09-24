use std::path::PathBuf;

use mai_core::deploy::{
    DeployError, HookResult, Os, ProbeStore, Remote, Shell, hooks_result, normalize_arch,
    parse_hash, parse_hook_lines, parse_uname, sha256_hex,
};
use mai_core::ssh::client::ExecOutput;

fn remote(os: Os, shell: Shell, home: &str) -> Remote {
    Remote {
        os,
        arch: "x86_64".into(),
        shell,
        home: home.into(),
    }
}

#[test]
fn uname_parsing() {
    assert_eq!(
        parse_uname("Darwin arm64\n"),
        Some((Os::MacOs, "aarch64".into()))
    );
    assert_eq!(
        parse_uname("Linux x86_64"),
        Some((Os::Linux, "x86_64".into()))
    );
    assert_eq!(
        parse_uname("Linux aarch64"),
        Some((Os::Linux, "aarch64".into()))
    );
    assert_eq!(parse_uname("FreeBSD amd64"), None);
    assert_eq!(parse_uname("Linux riscv64"), None);
    assert_eq!(parse_uname(""), None);
}

#[test]
fn windows_arch_names() {
    assert_eq!(normalize_arch("AMD64\r\n").as_deref(), Some("x86_64"));
    assert_eq!(normalize_arch("ARM64").as_deref(), Some("aarch64"));
    assert_eq!(normalize_arch("x86"), None);
}

#[test]
fn targets_for_supported_platforms() {
    let mut r = remote(Os::MacOs, Shell::Posix, "/Users/a");
    r.arch = "aarch64".into();
    assert_eq!(r.target(), Some("aarch64-apple-darwin"));
    assert_eq!(
        remote(Os::Linux, Shell::Posix, "/h").target(),
        Some("x86_64-unknown-linux-musl")
    );
    assert_eq!(
        remote(Os::Windows, Shell::Cmd, "C:\\U").target(),
        Some("x86_64-pc-windows-msvc")
    );
    let mut win_arm = remote(Os::Windows, Shell::Cmd, "C:\\U");
    win_arm.arch = "aarch64".into();
    assert_eq!(win_arm.target(), None);
}

#[test]
fn probe_paths_and_invocation_per_shell() {
    let unix = remote(Os::Linux, Shell::Posix, "/home/o'neil");
    assert_eq!(unix.probe_path(".mai"), "/home/o'neil/.mai/bin/mai-probe");
    assert_eq!(
        unix.invoke(&unix.probe_path(".mai"), "--version"),
        "'/home/o'\\''neil/.mai/bin/mai-probe' --version"
    );
    let cmd = remote(Os::Windows, Shell::Cmd, "C:\\Users\\x");
    let p = cmd.probe_path(".mai");
    assert_eq!(p, "C:\\Users\\x\\.mai\\bin\\mai-probe.exe");
    assert_eq!(
        cmd.invoke(&p, "serve"),
        "\"C:\\Users\\x\\.mai\\bin\\mai-probe.exe\" serve"
    );
    let ps = remote(Os::Windows, Shell::PowerShell, "C:\\Users\\x");
    assert_eq!(
        ps.invoke(&p, "serve"),
        "& 'C:\\Users\\x\\.mai\\bin\\mai-probe.exe' serve"
    );
}

#[test]
fn hash_commands_per_platform() {
    let mac = remote(Os::MacOs, Shell::Posix, "/Users/a");
    assert_eq!(mac.hash_command("/p"), "shasum -a 256 '/p'");
    let linux = remote(Os::Linux, Shell::Posix, "/h");
    assert_eq!(linux.hash_command("/p"), "sha256sum '/p'");
    let cmd = remote(Os::Windows, Shell::Cmd, "C:\\U");
    assert_eq!(
        cmd.hash_command("C:\\p.exe"),
        "certutil -hashfile \"C:\\p.exe\" SHA256"
    );
    let ps = remote(Os::Windows, Shell::PowerShell, "C:\\U");
    assert_eq!(
        ps.hash_command("C:\\p.exe"),
        "(Get-FileHash -Algorithm SHA256 'C:\\p.exe').Hash"
    );
}

const HASH: &str = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";

#[test]
fn hash_output_formats() {
    assert_eq!(
        parse_hash(&format!("{HASH}  /home/a/.mai/bin/mai-probe\n")).as_deref(),
        Some(HASH)
    );
    let certutil = format!(
        "SHA256 hash of C:\\p.exe:\r\n{}\r\nCertUtil: -hashfile command completed successfully.\r\n",
        HASH.to_uppercase()
    );
    assert_eq!(parse_hash(&certutil).as_deref(), Some(HASH));
    assert_eq!(parse_hash("sha256sum: /x: No such file or directory"), None);
    assert_eq!(sha256_hex(b"test"), HASH);
}

#[test]
fn probe_store_layout() {
    let store = ProbeStore {
        dir: PathBuf::from("probes"),
    };
    assert_eq!(
        store.binary("aarch64-apple-darwin"),
        PathBuf::from("probes")
            .join("aarch64-apple-darwin")
            .join("mai-probe")
    );
    assert_eq!(
        store.binary("x86_64-pc-windows-msvc"),
        PathBuf::from("probes")
            .join("x86_64-pc-windows-msvc")
            .join("mai-probe.exe")
    );
}

#[test]
fn install_hooks_output() {
    let out = "\
{\"agent\":\"claude\",\"outcome\":\"installed\",\"path\":\"/h/.claude/settings.json\"}
not json
{\"agent\":\"codex\",\"outcome\":\"installed\",\"path\":\"/h/.codex/hooks.json\",\"note\":\"run /hooks\"}
{\"agent\":\"x\",\"outcome\":\"skipped\",\"reason\":\"agent config dir not found\"}
";
    let r = parse_hook_lines(out);
    assert_eq!(r.len(), 3);
    assert_eq!(
        r[1],
        HookResult {
            agent: "codex".into(),
            outcome: "installed".into(),
            path: Some("/h/.codex/hooks.json".into()),
            note: Some("run /hooks".into()),
            reason: None,
            error: None,
        }
    );
    assert_eq!(r[2].reason.as_deref(), Some("agent config dir not found"));
}

#[test]
fn failing_install_hooks_is_reported_not_swallowed() {
    let out = ExecOutput {
        status: Some(1),
        stdout: b"{\"agent\":\"claude\",\"outcome\":\"installed\"}\n".to_vec(),
        stderr: b"permission denied".to_vec(),
    };
    let err = hooks_result(&out).unwrap_err();
    assert_eq!(
        err,
        DeployError::Hooks {
            status: Some(1),
            stderr: "permission denied".into(),
        }
    );
}

#[test]
fn missing_exit_status_is_also_reported() {
    let out = ExecOutput {
        status: None,
        stdout: Vec::new(),
        stderr: b"connection dropped".to_vec(),
    };
    let err = hooks_result(&out).unwrap_err();
    assert_eq!(
        err,
        DeployError::Hooks {
            status: None,
            stderr: "connection dropped".into(),
        }
    );
}

#[test]
fn successful_install_hooks_parses_output() {
    let out = ExecOutput {
        status: Some(0),
        stdout: b"{\"agent\":\"claude\",\"outcome\":\"installed\"}\n".to_vec(),
        stderr: Vec::new(),
    };
    let hooks = hooks_result(&out).unwrap();
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].agent, "claude");
}
