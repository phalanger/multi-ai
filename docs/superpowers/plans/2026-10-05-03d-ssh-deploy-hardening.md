# 03d SSH 与部署加固 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 处理 02/03 遗留的 SSH 与部署加固项：自实现容错的 known_hosts 解析
（哈希主机名、通配、`@revoked`）、ssh config 的 `Match` 块与嵌套 ProxyJump、
认证（先探测公钥再解密私钥、agent 去重与 partial success、keyboard-interactive
轮次上限、口令钥匙串键规范化、钥匙串失败上报）、部署（探测哨兵、上传后校验、
改名挪开旧文件再替换），并把不影响连接的提示（notes）一路交给 UI。

**Architecture:** `ssh::hostkey` 改为逐行解析 known_hosts（ssh-key 的
`KnownHosts` 只用来解析单行）；`ssh::signer` 提供按需解密的
`FileSigner`（实现 `russh::Signer`），`client::authenticate` 用
`authenticate_publickey_with` 先发公钥探测，服务器回 PK_OK 后才解密签名；
`swap` 模块的 `swap_in` 对本机（tokio::fs）与 SFTP 两种文件系统实现同一套
“挪开-改名-恢复”替换；提示经 `SshSession::notes` → `Opened.notes` →
`HostEvent::Notes` → `Update::Notes` 传递。

**Tech Stack:** Rust 1.92（edition 2024）、tokio、russh 0.63（含其 re-export 的
ssh-key）、russh-sftp 3、新增 hmac 0.13、sha1 0.11、signature 3；
既有的 mai-core / mai-protocol / mai-probe。

**Spec:** `docs/superpowers/specs/2026-09-23-multi-ai-monitor-design.md`
（第 3.1、3.2、3.3、4.2、11 节，已按本计划更新）；遗留事项
`docs/superpowers/plans/2026-09-24-02-followups.md`。

## Global Constraints

- 源码（代码、字符串、注释）只含 ASCII；测试、示例与文档可用中文。
- 零警告：`cargo build` 无警告；
  `cargo clippy --workspace --all-targets --locked -- -D warnings` 通过。
- 本计划给出的每个文件内容都已在 Windows 上逐任务编译、测试（各任务结束时的
  测试总数见各任务；最终 243 个），在 CI 三平台通过、5 个探针目标构建成功；
  部署替换已在 Windows 本机（另一个 client 的探针正锁住旧 exe）与 Ubuntu（SSH，
  用 CI 产出的探针，旧版本替换为新版本）验证。**逐字写入**。
  用编辑器或文件写入工具写，**不要用 Git Bash 的 heredoc**（会把 `\\` 压成 `\`）。
- 同一文件在多个任务中出现时，每次给出的都是该任务结束时的完整内容，直接整体
  替换；前面任务的版本是为了让每个任务单独可编译、可测试。
- 格式：本计划的文件已按 `rustfmt --edition 2024` 格式化；不要对任何 `lib.rs`
  或 `tracker.rs` 运行 rustfmt（会连带改动其他模块）。
- 在 Git Bash 中运行以 `/` 开头的参数时设 `MSYS_NO_PATHCONV=1`。
- 测试不得连接外部主机，不得读写真实的系统钥匙串、`~/.ssh`、`~/.mai`、
  `~/.claude`、`~/.codex`。钥匙串相关测试只用内存实现（如 `ReadOnlyStore`）。
- 远程主机（Mac、Ubuntu）只用于测试：**不在远程主机上构建或打包**；
  远程端到端验证使用 GitHub Actions `probes` 产物中的探针。
- 端到端只使用 `~/.mai-e2e`（monitor 示例不安装 hook），结束后删除；不创建、
  不 attach、不修改任何 zellij session，**绝不**动用户的 `work` session。
- Mac 端到端只在用户明确同意使用 Mac 时进行。
- 每次提交信息末尾空一行加：
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
- 分支：`feat/03d-ssh-deploy-hardening`，从 `main` 创建；本项目只有一名开发者，
  完成后本地合并到 `main` 并直接推送，不开 PR。

## Review Focus

- **服务器不接受的加密私钥不能弹窗要口令**：先发公钥探测，PK_OK 之后才解密
  （Task 3 `rejected_encrypted_key_is_never_decrypted`）；用户拒绝输入口令时
  继续尝试密码（`refused_passphrase_moves_on_to_password`）。
- **`@revoked` 优先**：同一密钥在别处是已知的也要拒绝；吊销只针对该密钥
  （Task 1 `revoked_key_is_refused_even_if_known`）。
- **坏行与读不出的文件要区别对待**：坏行跳过（`bad_lines_are_skipped_not_fatal`），
  I/O 错误仍拒绝（`unreadable_file_is_refused`）。
- **`Match` 块下的设置不能落到前一个 `Host` 上**（Task 2
  `match_blocks_are_ignored_not_misattributed`），且要有提示（`match_blocks_are_reported`、
  Task 5 `connect_notes_are_reported_after_hello`）。
- **替换探针时绝不先删旧文件**：Windows 上旧 exe 正在运行时仍能替换
  （Task 4 `running_executable_is_replaced_on_windows`）；新文件改名失败时旧文件
  恢复原位（`missing_upload_keeps_the_old_binary`）；上传内容与内置二进制
  SHA-256 不一致时不替换（`deploy::upload`，只有代码审查与端到端覆盖）。
- **钥匙串失败不再静默**（Task 3 `keychain_failure_is_reported_not_swallowed`）。
- **keyboard-interactive 不会无限循环**（Task 3 `endless_keyboard_interactive_gives_up`）。

## 遗留事项的处理

| 编号 | 处理 |
| --- | --- |
| B4 | `Match` 块整块忽略并提示（Task 2、5） |
| B5 | 口令钥匙串键先规范化路径（Task 3） |
| B6 | `@revoked` 拒绝、`@cert-authority` 忽略（Task 1） |
| B7 | keyboard-interactive 最多 10 轮；无应答不超时，设计如此（Task 3） |
| B8 | 自实现逐行容错解析，支持哈希主机名（Task 1） |
| B9 | 先探测公钥再解密；从文件试过的公钥不经 agent 重试（Task 3） |
| B10 | `learn` 去重（Task 1）；两个应用同时确认仍会各自弹窗 |
| B11 | 探测输出包在哨兵之间（Task 4） |
| B12 | cmd 下 `%USERPROFILE%` 未定义时报错（Task 4） |
| B13 | 经跳板机的进程内集成测试（Task 3）；真实网络未验证 |
| B14 | 跳板主机自身的 `ProxyJump` 不跟随，与 `connect` 一致（Task 2） |
| B16 | agent 的 partial success；钥匙串失败作为提示（Task 3、5） |
| B18 | 上传后在主机上校验 SHA-256（Task 4）；SFTP 不可用时的 exec 回退仍未实现 |
| B19 | 旧文件改名挪开再替换（Task 4） |
| B1、B20 | 需要本机 Windows sshd 的公钥登录，配置后再验证（不在本计划） |
| B2、B3、B15、B17、B23 | 需要特定环境或低优先级，不在本计划（Task 6 记录） |
| B25-B33 | 终端加固，留给 03e |

## 文件结构

```text
crates/mai-core/
  Cargo.toml                 + hmac 0.13、sha1 0.11、signature 3
  src/ssh/hostkey.rs         逐行容错解析；host_name、host_matches；Revoked；learn 去重
  src/ssh/config.rs          config_warnings；去掉 Match 块；跳板主机不递归
  src/ssh/signer.rs (新)     public_key_of、load_key、FileSigner（按需解密签名）
  src/ssh/auth.rs            passphrase_key 规范化路径
  src/ssh/client.rs          HostKeyRevoked；authenticate 改写；SshSession::notes
  src/ssh/mod.rs             + signer
  src/swap.rs (新)           BinFiles、swap_in、LocalFiles、SftpFiles
  src/deploy.rs              哨兵探测；上传后校验；swap_in
  src/connect.rs             HostKeyRevoked 映射；place_binary 改为 async + swap_in；notes
  src/host.rs                Problem::HostKeyRevoked；Opened.notes；HostEvent::Notes
  src/monitor.rs             Update::Notes
  src/lib.rs                 + swap
  tests/hostkey.rs、ssh_config.rs、ssh_session.rs、deploy.rs、connect.rs、
  hosts.rs、monitor.rs；tests/swap.rs (新)
```

---

### Task 1: known_hosts 容错解析与 `@revoked`（B8、B6、B10）

**Files:**

- Modify: `crates/mai-core/Cargo.toml`、`Cargo.lock`、`crates/mai-core/src/ssh/hostkey.rs`、
  `crates/mai-core/src/ssh/client.rs`、`crates/mai-core/src/host.rs`、
  `crates/mai-core/src/connect.rs`
- Test: `crates/mai-core/tests/hostkey.rs`、`crates/mai-core/tests/connect.rs`

**Interfaces:**

- Produces:

  ```rust
  // ssh::hostkey
  pub enum HostKeyStatus {
      Known, Unknown,
      Changed { file: PathBuf, line: usize },
      Revoked { file: PathBuf, line: usize },
      Unreadable { file: PathBuf, error: String },
  }
  pub fn host_name(host: &str, port: u16) -> String      // "h" 或 "[h]:p"，小写
  pub fn host_matches(patterns: &HostPatterns, name: &str) -> bool
  pub fn check(files: &[PathBuf], host: &str, port: u16, key: &PublicKey) -> HostKeyStatus
  pub fn learn(file: &Path, host: &str, port: u16, key: &PublicKey) -> Result<(), String>
  // ssh::client
  SshError::HostKeyRevoked { host: String, port: u16, file: PathBuf, line: usize }
  // host
  Problem::HostKeyRevoked { file: PathBuf, line: usize }   // needs_auth 为 true
  ```

  `check` 的优先级：任一文件 `Revoked` > 任一文件 `Changed` > `Known` > `Unknown`；
  `learn` 在文件已有同一条记录时不写。

- [ ] **Step 1: `crates/mai-core/Cargo.toml` 最终内容**

```toml
[package]
name = "mai-core"
version.workspace = true
edition.workspace = true

[dependencies]
hmac = "0.13"
keyring = "4"
mai-protocol = { workspace = true }
portable-pty = "0.9"
russh = { version = "0.63", default-features = false, features = ["flate2", "ring", "rsa"] }
russh-sftp = "3"
serde = { workspace = true }
serde_json = { workspace = true }
sha1 = "0.11"
sha2 = "0.11"
signature = "3"
ssh2-config = "0.8"
tokio = { version = "1", features = ["full"] }

[dev-dependencies]
rpassword = "7"
ssh-key = { version = "=0.7.0-rc.11", features = ["getrandom", "ed25519", "encryption"] }
tempfile = "3"
tokio = { version = "1", features = ["full", "test-util"] }

[lints]
workspace = true
```

Run: `cargo build -p mai-core`（不加 `--locked`，以更新 `Cargo.lock`）
Expected: `git diff Cargo.lock` 只在 `mai-core` 的依赖列表里多出 `"hmac"`、`"sha1"`、
`"signature"` 三行，没有新增或升级任何包。

- [ ] **Step 2: 写失败测试 `crates/mai-core/tests/hostkey.rs`（完整内容）**

```rust
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
```

`crates/mai-core/tests/connect.rs`（完整内容，新增 `revoked_host_key_needs_the_user`）：

```rust
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

#[test]
fn revoked_host_key_needs_the_user() {
    assert_eq!(
        ssh_open_error(SshError::HostKeyRevoked {
            host: "h".into(),
            port: 22,
            file: PathBuf::from("kh"),
            line: 4,
        }),
        OpenError::NeedsUser(Problem::HostKeyRevoked {
            file: PathBuf::from("kh"),
            line: 4
        })
    );
}
```

- [ ] **Step 3: 运行，确认失败**

Run: `cargo test -p mai-core --test hostkey --test connect`
Expected: 编译失败（`host_name`、`HostKeyStatus::Revoked`、`SshError::HostKeyRevoked`
等不存在）。

- [ ] **Step 4: 写 `crates/mai-core/src/ssh/hostkey.rs`（完整内容）**

```rust
//! Server host-key verification against OpenSSH `known_hosts` files.
//!
//! Files are parsed line by line and leniently: a line that cannot be
//! parsed (bad base64, unsupported key type, invalid UTF-8) is skipped
//! like OpenSSH does, instead of making the whole file unusable. Host
//! patterns support `*`/`?` globs, `!` negation, `[host]:port` and hashed
//! names (`|1|salt|hash`, HMAC-SHA1). `@revoked` entries refuse a key;
//! `@cert-authority` entries are ignored (host certificates are refused).

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use hmac::{Hmac, KeyInit, Mac};
use russh::keys::ssh_key::known_hosts::{HostPatterns, KnownHosts, Marker};
use russh::keys::{HashAlg, PublicKey};
use sha1::Sha1;

/// Result of looking a host key up in the known_hosts files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyStatus {
    /// Some file records exactly this key for the host.
    Known,
    /// No file has a key of this algorithm for the host.
    Unknown,
    /// A file records a different key of the same algorithm: refuse.
    Changed { file: PathBuf, line: usize },
    /// A file marks this key `@revoked` for the host: refuse.
    Revoked { file: PathBuf, line: usize },
    /// A file exists but cannot be read (I/O error): refuse rather than
    /// treat it as missing.
    Unreadable { file: PathBuf, error: String },
}

/// `SHA256:<base64>` as printed by `ssh-keygen -l`.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// The name OpenSSH records for `host:port`: the lowercased host, or
/// `[host]:port` when the port is not 22.
pub fn host_name(host: &str, port: u16) -> String {
    let host = host.to_ascii_lowercase();
    if port == 22 {
        host
    } else {
        format!("[{host}]:{port}")
    }
}

/// Glob match with `*` (any run) and `?` (one character).
fn glob(pattern: &[char], text: &[char]) -> bool {
    match pattern.split_first() {
        None => text.is_empty(),
        Some(('*', rest)) => (0..=text.len()).any(|i| glob(rest, &text[i..])),
        Some(('?', rest)) => !text.is_empty() && glob(rest, &text[1..]),
        Some((c, rest)) => text.first() == Some(c) && glob(rest, &text[1..]),
    }
}

fn hashed_matches(salt: &[u8], hash: &[u8; 20], name: &str) -> bool {
    let Ok(mut mac) = <Hmac<Sha1> as KeyInit>::new_from_slice(salt) else {
        return false;
    };
    mac.update(name.as_bytes());
    mac.verify_slice(hash).is_ok()
}

/// Does an entry's host field apply to `name` (from `host_name`)? A list
/// applies when some pattern matches and no negated pattern does.
pub fn host_matches(patterns: &HostPatterns, name: &str) -> bool {
    match patterns {
        HostPatterns::HashedName { salt, hash } => hashed_matches(salt, hash, name),
        HostPatterns::Patterns(list) => {
            let text: Vec<char> = name.chars().collect();
            let mut matched = false;
            for p in list {
                let (negated, p) = match p.strip_prefix('!') {
                    Some(rest) => (true, rest),
                    None => (false, p.as_str()),
                };
                let p: Vec<char> = p.to_ascii_lowercase().chars().collect();
                if glob(&p, &text) {
                    if negated {
                        return false;
                    }
                    matched = true;
                }
            }
            matched
        }
    }
}

/// What one file says about `key` for `name`.
#[derive(Debug, Default, PartialEq, Eq)]
struct FileVerdict {
    known: bool,
    changed: Option<usize>,
    revoked: Option<usize>,
}

fn scan(text: &str, name: &str, key: &PublicKey) -> FileVerdict {
    let mut v = FileVerdict::default();
    for (i, line) in text.lines().enumerate() {
        // One line at a time, so a bad line only skips itself.
        let Some(Ok(entry)) = KnownHosts::new(line).next() else {
            continue;
        };
        if !host_matches(entry.host_patterns(), name) {
            continue;
        }
        let same_key = entry.public_key().key_data() == key.key_data();
        match entry.marker() {
            Some(Marker::Revoked) => {
                if same_key {
                    v.revoked.get_or_insert(i + 1);
                }
            }
            Some(Marker::CertAuthority) => {}
            None => {
                if same_key {
                    v.known = true;
                } else if entry.public_key().algorithm() == key.algorithm() {
                    v.changed.get_or_insert(i + 1);
                }
            }
        }
    }
    v
}

/// Check `key` for `host:port` in every file. A revoked or changed key in
/// any file wins over a match elsewhere. Missing files are skipped; a file
/// that exists but cannot be read is refused.
pub fn check(files: &[PathBuf], host: &str, port: u16, key: &PublicKey) -> HostKeyStatus {
    let name = host_name(host, port);
    let mut known = false;
    let mut changed = None;
    for file in files.iter().filter(|f| f.exists()) {
        let bytes = match std::fs::read(file) {
            Ok(b) => b,
            Err(e) => {
                return HostKeyStatus::Unreadable {
                    file: file.clone(),
                    error: e.to_string(),
                };
            }
        };
        let v = scan(&String::from_utf8_lossy(&bytes), &name, key);
        if let Some(line) = v.revoked {
            return HostKeyStatus::Revoked {
                file: file.clone(),
                line,
            };
        }
        if changed.is_none()
            && let Some(line) = v.changed
        {
            changed = Some((file.clone(), line));
        }
        known |= v.known;
    }
    match (changed, known) {
        (Some((file, line)), _) => HostKeyStatus::Changed { file, line },
        (None, true) => HostKeyStatus::Known,
        (None, false) => HostKeyStatus::Unknown,
    }
}

/// Append `key` for `host:port` to `file` (created with parents if
/// missing), unless the file already records exactly this key for the
/// host. The line is `<host_name> <type> <base64>`.
pub fn learn(file: &Path, host: &str, port: u16, key: &PublicKey) -> Result<(), String> {
    let err = |e: io::Error| format!("{}: {e}", file.display());
    let existing = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(err(e)),
    };
    let name = host_name(host, port);
    if scan(&String::from_utf8_lossy(&existing), &name, key).known {
        return Ok(());
    }
    let mut bare = key.clone();
    bare.set_comment("");
    let encoded = bare
        .to_openssh()
        .map_err(|e| format!("{}: {e}", file.display()))?;
    let mut line = String::new();
    if !existing.is_empty() && !existing.ends_with(b"\n") {
        line.push('\n');
    }
    line.push_str(&format!("{name} {}\n", encoded.trim_end()));
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .map_err(err)
}
```

- [ ] **Step 5: `crates/mai-core/src/ssh/client.rs`（完整内容）**

新增 `SshError::HostKeyRevoked` 及其 `Display`，`Checker` 处理
`HostKeyStatus::Revoked`：

```rust
//! SSH sessions: connect (optionally through jump hosts), verify the host
//! key, authenticate, and run commands.
//!
//! Auth order (design 3.2): private key files (passphrase from the secret
//! store or prompt), ssh-agent, remembered password, prompted password,
//! keyboard-interactive. Methods the server does not offer are skipped.

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use russh::keys::{PrivateKey, PrivateKeyWithHashAlg, PublicKeyOrCertificate};
use russh::{ChannelMsg, MethodKind, MethodSet};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream};

use super::auth::{KbdPrompt, Prompter, SecretStore, passphrase_key, password_key};
use super::config::HostSpec;
use super::hostkey::{self, HostKeyStatus};
use crate::pty::{PtyIo, TermSize};
use crate::stderr::StderrTail;

/// Where host keys are checked and learned, and how long to wait.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Checked in order (e.g. `~/.ssh/known_hosts`, the app's own file).
    pub known_hosts: Vec<PathBuf>,
    /// Keys the user confirms are appended here.
    pub learn_to: PathBuf,
    /// TCP connect + key exchange timeout per hop.
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshError {
    /// TCP, key exchange, or transport failure, including a transport
    /// error raised while an authentication call was in flight (the server
    /// or network dropped the connection, not a rejected credential).
    /// Retryable: the 03b host manager reconnects on this.
    Connect(String),
    /// The user declined an unknown host key.
    HostKeyRejected {
        host: String,
        port: u16,
        fingerprint: String,
    },
    /// known_hosts has a different key for this host: never connect.
    HostKeyChanged {
        host: String,
        port: u16,
        file: PathBuf,
        line: usize,
    },
    /// known_hosts marks this host key `@revoked`: never connect.
    HostKeyRevoked {
        host: String,
        port: u16,
        file: PathBuf,
        line: usize,
    },
    /// No authentication method succeeded: the server ran out of methods
    /// to offer. Not retryable by reconnecting; needs the user to supply
    /// different credentials. The 03b host manager treats this as
    /// `AuthRequired` and does not auto-retry.
    Auth(String),
    Channel(String),
}

impl fmt::Display for SshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(m) => write!(f, "connect: {m}"),
            Self::HostKeyRejected {
                host,
                port,
                fingerprint,
            } => {
                write!(f, "host key for {host}:{port} not trusted ({fingerprint})")
            }
            Self::HostKeyChanged {
                host,
                port,
                file,
                line,
            } => write!(
                f,
                "HOST KEY CHANGED for {host}:{port} (see {}:{line}); refusing to connect",
                file.display()
            ),
            Self::HostKeyRevoked {
                host,
                port,
                file,
                line,
            } => write!(
                f,
                "host key for {host}:{port} is REVOKED (see {}:{line}); refusing to connect",
                file.display()
            ),
            Self::Auth(m) => write!(f, "authentication: {m}"),
            Self::Channel(m) => write!(f, "channel: {m}"),
        }
    }
}

impl std::error::Error for SshError {}

fn chan_err(e: impl fmt::Display) -> SshError {
    SshError::Channel(e.to_string())
}

/// A `russh::Error` raised by an authentication call is a transport
/// failure, not a rejected credential: russh only reports rejection
/// through `AuthResult::Failure`. `SshError::Auth` is reserved for
/// "no method succeeded" once every offered method has been tried.
fn connect_err(e: impl fmt::Display) -> SshError {
    SshError::Connect(e.to_string())
}

/// Why a host key was refused, filled in by the handler.
type Verdict = Arc<Mutex<Option<SshError>>>;

const IDLE: u8 = 0;
const PROMPTING: u8 = 1;
const ABANDONED: u8 = 2;

/// Coordination between the connect timeout and the host-key prompt.
/// `phase` moves IDLE -> PROMPTING -> IDLE (handler) or IDLE -> ABANDONED
/// (timeout); both use compare-and-swap, so a timed-out connection never
/// prompts or learns a key, and a prompt is never cut short by the timer.
#[derive(Default)]
struct HopState {
    phase: AtomicU8,
    /// Incremented when a prompt finishes; the timer then starts afresh.
    prompts_done: AtomicU64,
}

/// Files to check a host key against: `known_hosts`, plus `learn_to` if it
/// isn't already one of them. This way a caller whose `learn_to` is not
/// listed in `known_hosts` is not re-prompted on every connect for a key
/// it already learned and wrote there itself.
fn check_files(opts: &ConnectOptions) -> Vec<PathBuf> {
    let mut files = opts.known_hosts.clone();
    if !files.contains(&opts.learn_to) {
        files.push(opts.learn_to.clone());
    }
    files
}

pub struct Checker<P> {
    host: String,
    port: u16,
    opts: ConnectOptions,
    prompter: Arc<P>,
    verdict: Verdict,
    state: Arc<HopState>,
}

impl<P: Prompter> client::Handler for Checker<P> {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            self.refuse(SshError::Connect(
                "host certificates are not supported".into(),
            ));
            return Ok(false);
        };
        match hostkey::check(&check_files(&self.opts), &self.host, self.port, key) {
            HostKeyStatus::Known => Ok(true),
            HostKeyStatus::Changed { file, line } => {
                self.refuse(SshError::HostKeyChanged {
                    host: self.host.clone(),
                    port: self.port,
                    file,
                    line,
                });
                Ok(false)
            }
            HostKeyStatus::Revoked { file, line } => {
                self.refuse(SshError::HostKeyRevoked {
                    host: self.host.clone(),
                    port: self.port,
                    file,
                    line,
                });
                Ok(false)
            }
            HostKeyStatus::Unreadable { file, error } => {
                // Fail closed: never let an unparsable file fall through
                // to an Unknown-host prompt. No prompt is shown.
                self.refuse(SshError::Connect(format!(
                    "cannot read known_hosts file {}: {error}",
                    file.display()
                )));
                Ok(false)
            }
            HostKeyStatus::Unknown => {
                let claimed = self.state.phase.compare_exchange(
                    IDLE,
                    PROMPTING,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                if claimed.is_err() {
                    // The connection already timed out: do not prompt.
                    return Ok(false);
                }
                let fp = hostkey::fingerprint(key);
                let trusted = self
                    .prompter
                    .confirm_host_key(&self.host, self.port, &fp)
                    .await;
                // Decide and learn while still PROMPTING, so the timer
                // cannot abandon the connection in between.
                let accepted = if !trusted {
                    self.refuse(SshError::HostKeyRejected {
                        host: self.host.clone(),
                        port: self.port,
                        fingerprint: fp,
                    });
                    false
                } else if let Err(e) =
                    hostkey::learn(&self.opts.learn_to, &self.host, self.port, key)
                {
                    self.refuse(SshError::Connect(format!("cannot record host key: {e}")));
                    false
                } else {
                    true
                };
                self.state.prompts_done.fetch_add(1, Ordering::SeqCst);
                self.state.phase.store(IDLE, Ordering::SeqCst);
                Ok(accepted)
            }
        }
    }
}

impl<P> Checker<P> {
    fn refuse(&self, e: SshError) {
        if let Ok(mut v) = self.verdict.lock() {
            *v = Some(e);
        }
    }
}

/// Output of a finished remote command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub status: Option<u32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// An authenticated SSH connection. Keeps its jump-host connections alive.
pub struct SshSession<P: Prompter> {
    handle: Handle<Checker<P>>,
    _via: Option<Box<SshSession<P>>>,
}

fn client_config() -> Arc<client::Config> {
    Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    })
}

/// Connect to `spec`, hopping through `spec.jumps` (outermost first).
/// Jump hosts' own `ProxyJump` settings are not followed.
pub async fn connect<P: Prompter, S: SecretStore>(
    spec: &HostSpec,
    opts: &ConnectOptions,
    prompter: Arc<P>,
    secrets: &S,
) -> Result<SshSession<P>, SshError> {
    let mut via: Option<SshSession<P>> = None;
    for hop in spec.jumps.iter().chain(std::iter::once(spec)) {
        let session = connect_hop(hop, opts, prompter.clone(), secrets, via.take()).await?;
        via = Some(session);
    }
    via.ok_or_else(|| SshError::Connect("no hops".into()))
}

async fn connect_hop<P: Prompter, S: SecretStore>(
    hop: &HostSpec,
    opts: &ConnectOptions,
    prompter: Arc<P>,
    secrets: &S,
    via: Option<SshSession<P>>,
) -> Result<SshSession<P>, SshError> {
    let verdict: Verdict = Arc::default();
    let state = Arc::new(HopState::default());
    let checker = Checker {
        host: hop.host.clone(),
        port: hop.port,
        opts: opts.clone(),
        prompter: prompter.clone(),
        verdict: verdict.clone(),
        state: state.clone(),
    };
    let result = {
        let connecting = async {
            match &via {
                None => {
                    client::connect(client_config(), (hop.host.as_str(), hop.port), checker).await
                }
                Some(outer) => {
                    let ch = outer
                        .handle
                        .channel_open_direct_tcpip(
                            hop.host.clone(),
                            u32::from(hop.port),
                            "127.0.0.1",
                            0,
                        )
                        .await?;
                    client::connect_stream(client_config(), ch.into_stream(), checker).await
                }
            }
        };
        // The timeout covers TCP connect and key exchange only: it waits
        // while the host-key prompt is open, and starts a full period again
        // after a prompt finishes. Giving up marks the hop ABANDONED so the
        // detached russh session task can no longer prompt or learn a key.
        tokio::pin!(connecting);
        let mut seen = state.prompts_done.load(Ordering::SeqCst);
        loop {
            tokio::select! {
                r = &mut connecting => break r,
                _ = tokio::time::sleep(opts.timeout) => {
                    let done = state.prompts_done.load(Ordering::SeqCst);
                    if done != seen {
                        seen = done;
                        continue;
                    }
                    let gave_up = state.phase.compare_exchange(
                        IDLE,
                        ABANDONED,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    );
                    if gave_up.is_ok() {
                        return Err(SshError::Connect(format!(
                            "{}:{} timed out",
                            hop.host, hop.port
                        )));
                    }
                }
            }
        }
    };
    let mut handle = match result {
        Err(e) => {
            let refused = verdict.lock().ok().and_then(|mut v| v.take());
            return Err(refused
                .unwrap_or_else(|| SshError::Connect(format!("{}:{}: {e}", hop.host, hop.port))));
        }
        Ok(h) => h,
    };
    authenticate(&mut handle, hop, prompter.as_ref(), secrets).await?;
    Ok(SshSession {
        handle,
        _via: via.map(Box::new),
    })
}

fn offers(methods: &Option<MethodSet>, kind: MethodKind) -> bool {
    methods.as_ref().is_none_or(|m| m.contains(&kind))
}

/// Result of one authentication attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Fully authenticated.
    Done,
    /// This factor was accepted but the server wants more (e.g. 2FA).
    Partial,
    Failed,
}

/// Record the server's remaining methods and classify the result.
fn step(methods: &mut Option<MethodSet>, r: &client::AuthResult) -> Step {
    match r {
        client::AuthResult::Success => Step::Done,
        client::AuthResult::Failure {
            remaining_methods,
            partial_success,
        } => {
            *methods = Some(remaining_methods.clone());
            if *partial_success {
                Step::Partial
            } else {
                Step::Failed
            }
        }
    }
}

async fn authenticate<P: Prompter, S: SecretStore>(
    h: &mut Handle<Checker<P>>,
    spec: &HostSpec,
    prompter: &P,
    secrets: &S,
) -> Result<(), SshError> {
    let user = spec.user.as_str();
    let mut methods = match h.authenticate_none(user).await.map_err(connect_err)? {
        client::AuthResult::Success => return Ok(()),
        client::AuthResult::Failure {
            remaining_methods, ..
        } => Some(remaining_methods),
    };
    'keys: {
        if !offers(&methods, MethodKind::PublicKey) {
            break 'keys;
        }
        for path in spec.identity_files.iter().filter(|p| p.is_file()) {
            let Some(key) = load_key(path, prompter, secrets).await else {
                continue;
            };
            let hash = h
                .best_supported_rsa_hash()
                .await
                .map_err(connect_err)?
                .flatten();
            let r = h
                .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                .await
                .map_err(connect_err)?;
            match step(&mut methods, &r) {
                Step::Done => return Ok(()),
                Step::Partial => break 'keys,
                Step::Failed => {}
            }
        }
        if try_agent(h, user).await? {
            return Ok(());
        }
    }

    if offers(&methods, MethodKind::Password) {
        let key = password_key(&spec.alias);
        let mut accepted = false;
        if let Some(pw) = secrets.get(&key) {
            let r = h
                .authenticate_password(user, pw)
                .await
                .map_err(connect_err)?;
            match step(&mut methods, &r) {
                Step::Done => return Ok(()),
                Step::Partial => accepted = true,
                Step::Failed => {
                    let _ = secrets.delete(&key);
                }
            }
        }
        if !accepted
            && offers(&methods, MethodKind::Password)
            && let Some(s) = prompter.password(user, &spec.host).await
        {
            let r = h
                .authenticate_password(user, s.value.clone())
                .await
                .map_err(connect_err)?;
            let result = step(&mut methods, &r);
            if result != Step::Failed && s.remember {
                let _ = secrets.set(&key, &s.value);
            }
            if result == Step::Done {
                return Ok(());
            }
        }
    }

    if offers(&methods, MethodKind::KeyboardInteractive)
        && keyboard_interactive(h, spec, prompter).await?
    {
        return Ok(());
    }
    Err(SshError::Auth(format!(
        "no method succeeded for {user}@{}",
        spec.host
    )))
}

/// Load a private key; ask for (and optionally remember) its passphrase.
async fn load_key<P: Prompter, S: SecretStore>(
    path: &Path,
    prompter: &P,
    secrets: &S,
) -> Option<PrivateKey> {
    match russh::keys::load_secret_key(path, None) {
        Ok(k) => return Some(k),
        Err(russh::keys::Error::KeyIsEncrypted) => {}
        Err(_) => return None,
    }
    let key = passphrase_key(path);
    if let Some(pass) = secrets.get(&key) {
        if let Ok(k) = russh::keys::load_secret_key(path, Some(&pass)) {
            return Some(k);
        }
        let _ = secrets.delete(&key);
    }
    let s = prompter.passphrase(path).await?;
    let k = russh::keys::load_secret_key(path, Some(&s.value)).ok()?;
    if s.remember {
        let _ = secrets.set(&key, &s.value);
    }
    Some(k)
}

async fn agent_auth<P: Prompter, A>(
    h: &mut Handle<Checker<P>>,
    user: &str,
    mut agent: AgentClient<A>,
) -> Result<bool, SshError>
where
    A: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Ok(ids) = agent.request_identities().await else {
        return Ok(false);
    };
    for id in ids {
        let AgentIdentity::PublicKey { key, .. } = id else {
            continue;
        };
        let hash = h
            .best_supported_rsa_hash()
            .await
            .map_err(connect_err)?
            .flatten();
        if let Ok(r) = h
            .authenticate_publickey_with(user, key, hash, &mut agent)
            .await
            && r.success()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(unix)]
async fn try_agent<P: Prompter>(h: &mut Handle<Checker<P>>, user: &str) -> Result<bool, SshError> {
    match AgentClient::connect_env().await {
        Ok(agent) => agent_auth(h, user, agent).await,
        Err(_) => Ok(false),
    }
}

#[cfg(windows)]
async fn try_agent<P: Prompter>(h: &mut Handle<Checker<P>>, user: &str) -> Result<bool, SshError> {
    if let Ok(agent) = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await
        && agent_auth(h, user, agent).await?
    {
        return Ok(true);
    }
    match AgentClient::connect_pageant().await {
        Ok(agent) => agent_auth(h, user, agent).await,
        Err(_) => Ok(false),
    }
}

async fn keyboard_interactive<P: Prompter>(
    h: &mut Handle<Checker<P>>,
    spec: &HostSpec,
    prompter: &P,
) -> Result<bool, SshError> {
    let mut reply = h
        .authenticate_keyboard_interactive_start(spec.user.clone(), None)
        .await
        .map_err(connect_err)?;
    loop {
        match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let asks: Vec<KbdPrompt> = prompts
                    .iter()
                    .map(|p| KbdPrompt {
                        text: p.prompt.clone(),
                        echo: p.echo,
                    })
                    .collect();
                let answers = if asks.is_empty() {
                    Vec::new()
                } else {
                    match prompter
                        .keyboard_interactive(&name, &instructions, &asks)
                        .await
                    {
                        Some(a) => a,
                        None => return Ok(false),
                    }
                };
                reply = h
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(connect_err)?;
            }
        }
    }
}

impl<P: Prompter> SshSession<P> {
    /// Run `command` to completion, collecting stdout, stderr and status.
    pub async fn exec(&self, command: &str) -> Result<ExecOutput, SshError> {
        let mut ch = self.handle.channel_open_session().await.map_err(chan_err)?;
        ch.exec(true, command).await.map_err(chan_err)?;
        let mut out = ExecOutput::default();
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => out.stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => out.status = Some(exit_status),
                _ => {}
            }
        }
        Ok(out)
    }

    /// Start `command` and hand back its channel for streaming (stdin via
    /// `data`, stdout via `wait`). `env` is requested first; servers that
    /// do not accept a variable ignore it.
    pub async fn open_exec(
        &self,
        command: &str,
        env: &[(&str, &str)],
    ) -> Result<russh::Channel<client::Msg>, SshError> {
        let ch = self.handle.channel_open_session().await.map_err(chan_err)?;
        for (k, v) in env {
            ch.set_env(false, *k, *v).await.map_err(chan_err)?;
        }
        ch.exec(true, command).await.map_err(chan_err)?;
        Ok(ch)
    }

    /// Like `open_exec`, but split into a stdout byte stream and a stdin
    /// writer, with stderr collected into `stderr`.
    pub async fn open_exec_split(
        &self,
        command: &str,
        env: &[(&str, &str)],
        stderr: Arc<StderrTail>,
    ) -> Result<(DuplexStream, Box<dyn AsyncWrite + Send + Unpin>), SshError> {
        let (mut read, write) = self.open_exec(command, env).await?.split();
        let writer: Box<dyn AsyncWrite + Send + Unpin> = Box::new(write.make_writer());
        let (mut stdout, reader) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            // The write half lives as long as the channel is read.
            let _write = write;
            while let Some(msg) = read.wait().await {
                match msg {
                    ChannelMsg::Data { data } => {
                        if stdout.write_all(&data).await.is_err() {
                            break;
                        }
                    }
                    ChannelMsg::ExtendedData { data, .. } => stderr.push(&data),
                    ChannelMsg::Eof | ChannelMsg::Close => break,
                    _ => {}
                }
            }
            stderr.finish();
        });
        Ok((reader, writer))
    }

    /// Run a command in a PTY of `size` (terminal type `xterm-256color`).
    /// The variables in `env` are requested first and each answer is
    /// awaited; `command` receives whether all were accepted, so it can
    /// fall back to setting them in the command line.
    pub async fn open_pty(
        &self,
        size: TermSize,
        env: &[(&str, &str)],
        command: impl FnOnce(bool) -> String,
    ) -> Result<PtyIo, SshError> {
        let ch = self.handle.channel_open_session().await.map_err(chan_err)?;
        super::pty::start(ch, size, env, command).await
    }

    /// Open an SFTP session. Relative paths are relative to the login
    /// directory (the home directory on OpenSSH servers).
    pub async fn sftp(&self) -> Result<russh_sftp::client::SftpSession, SshError> {
        let ch = self.handle.channel_open_session().await.map_err(chan_err)?;
        ch.request_subsystem(true, "sftp").await.map_err(chan_err)?;
        russh_sftp::client::SftpSession::new(ch.into_stream())
            .await
            .map_err(chan_err)
    }

    pub async fn close(self) {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }
}

// `step` is tested here because the in-process russh test server never
// sends `partial_success = true` (russh 0.63 resets it), so the 2FA path
// cannot be driven end-to-end in tests/ssh_session.rs.
#[cfg(test)]
mod tests {
    use super::*;

    fn failure(methods: &[MethodKind], partial: bool) -> client::AuthResult {
        client::AuthResult::Failure {
            remaining_methods: MethodSet::from(methods),
            partial_success: partial,
        }
    }

    #[test]
    fn step_classifies_success_partial_and_failure() {
        let mut methods = None;
        assert_eq!(step(&mut methods, &client::AuthResult::Success), Step::Done);
        assert_eq!(methods, None);

        let r = failure(&[MethodKind::KeyboardInteractive], true);
        assert_eq!(step(&mut methods, &r), Step::Partial);
        assert!(!offers(&methods, MethodKind::Password));
        assert!(offers(&methods, MethodKind::KeyboardInteractive));

        let r = failure(&[MethodKind::Password], false);
        assert_eq!(step(&mut methods, &r), Step::Failed);
        assert!(offers(&methods, MethodKind::Password));
    }

    #[test]
    fn unknown_method_set_offers_everything() {
        assert!(offers(&None, MethodKind::PublicKey));
        assert!(offers(&None, MethodKind::Password));
    }
}
```

- [ ] **Step 6: `crates/mai-core/src/host.rs`（完整内容）**

```rust
//! One task per monitored host keeps its probe connection alive: connect,
//! deploy, start `serve`, relay messages, and reconnect with backoff.
//! Because only this task connects and deploys to its host, those steps
//! never run concurrently for one host.

use std::any::Any;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mai_protocol::{AppMsg, ProbeMsg};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

use crate::deploy::HookResult;
use crate::link::{HelloInfo, LinkError, ProbeIo, ProbeLink};
use crate::stderr::StderrTail;
use crate::term::TermTransport;

/// How the app reaches a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKind {
    /// This machine: the probe runs as a child process.
    Local,
    /// An `ssh` target: a `~/.ssh/config` alias or `[user@]host[:port]`.
    Ssh { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfig {
    /// Stable id, unique among hosts (used in `AgentKey::host_id`).
    pub id: String,
    pub kind: HostKind,
    /// zellij binary on the host when it is not found automatically.
    pub zellij: Option<String>,
}

/// Why a connection waits for the user instead of retrying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// No authentication method succeeded.
    Auth(String),
    /// The user declined the host key.
    HostKeyRejected,
    /// The host key differs from a recorded one.
    HostKeyChanged { file: PathBuf, line: usize },
    /// A known_hosts file marks the host key `@revoked`.
    HostKeyRevoked { file: PathBuf, line: usize },
    /// The probe could not be put on the host.
    Deploy(String),
    /// The host's configuration cannot be used (e.g. ssh config error).
    Config(String),
    /// The deployed probe speaks another protocol version.
    Protocol { probe: u32, app: u32 },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(m) => write!(f, "authentication failed: {m}"),
            Self::HostKeyRejected => f.write_str("host key not accepted"),
            Self::HostKeyChanged { file, line } => write!(
                f,
                "HOST KEY CHANGED (recorded in {} line {line})",
                file.display()
            ),
            Self::HostKeyRevoked { file, line } => write!(
                f,
                "host key REVOKED (marked in {} line {line})",
                file.display()
            ),
            Self::Deploy(m) => write!(f, "probe deployment failed: {m}"),
            Self::Config(m) => write!(f, "configuration error: {m}"),
            Self::Protocol { probe, app } => {
                write!(
                    f,
                    "probe protocol {probe} does not match app protocol {app}"
                )
            }
        }
    }
}

/// Why `Connector::open` failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Transient (network, timeout): retried with backoff.
    Retry(String),
    /// Needs the user: not retried until `HostCommand::Retry`.
    NeedsUser(Problem),
}

/// A started probe.
pub struct Opened {
    pub io: ProbeIo,
    /// Result of `install-hooks`. A failure only degrades detection to
    /// screen scraping, so it does not fail the connection.
    pub hooks: Result<Vec<HookResult>, String>,
    /// Kept alive while the probe runs (SSH session, child process).
    pub keep: Box<dyn Any + Send>,
    /// The probe's stderr, to explain why it stopped.
    pub stderr: Arc<StderrTail>,
}

/// `reason`, followed by the end of the probe's stderr if it wrote any.
async fn with_stderr(reason: String, stderr: &StderrTail) -> String {
    match stderr.summary().await {
        Some(tail) => format!("{reason} (probe stderr: {tail})"),
        None => reason,
    }
}

/// Reaches hosts: the probe connection (`open`: connect, deploy, start
/// `serve`) and the terminal connection (`open_terminals`).
pub trait Connector: Send + Sync + 'static {
    type Terminals: TermTransport;

    fn open(&self, host: &HostConfig) -> impl Future<Output = Result<Opened, OpenError>> + Send;

    fn open_terminals(
        &self,
        host: &HostConfig,
    ) -> impl Future<Output = Result<Self::Terminals, OpenError>> + Send;
}

/// State of one connection of a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Up,
    /// Down; the next attempt starts after `retry_in`.
    Retrying {
        retry_in: Duration,
        reason: String,
    },
    /// Down until the user acts (`HostCommand::Retry`).
    NeedsUser(Problem),
}

/// Overall host state shown in the UI (design 3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostState {
    Connecting,
    Online,
    /// Only one of the two connections is up.
    Degraded,
    Offline,
    AuthRequired,
}

fn needs_auth(c: &ConnState) -> bool {
    matches!(
        c,
        ConnState::NeedsUser(
            Problem::Auth(_)
                | Problem::HostKeyRejected
                | Problem::HostKeyChanged { .. }
                | Problem::HostKeyRevoked { .. }
        )
    )
}

/// Host state from its probe connection and, once the terminal
/// connection exists, the terminal connection (`None` until then).
pub fn host_state(probe: &ConnState, term: Option<&ConnState>) -> HostState {
    let conns: Vec<&ConnState> = std::iter::once(probe).chain(term).collect();
    let up = conns.iter().filter(|c| ***c == ConnState::Up).count();
    if up == conns.len() {
        HostState::Online
    } else if up > 0 {
        HostState::Degraded
    } else if conns.iter().any(|c| needs_auth(c)) {
        HostState::AuthRequired
    } else if conns.iter().any(|c| **c == ConnState::Connecting) {
        HostState::Connecting
    } else {
        HostState::Offline
    }
}

/// Reconnect delays: 1 s doubling to 60 s (design 3.4).
#[derive(Debug, Clone)]
pub struct Backoff {
    next: Duration,
}

impl Backoff {
    pub const FIRST: Duration = Duration::from_secs(1);
    pub const MAX: Duration = Duration::from_secs(60);

    /// The delay to wait now; the following one doubles.
    pub fn next_delay(&mut self) -> Duration {
        let d = self.next;
        self.next = (d * 2).min(Self::MAX);
        d
    }

    pub fn reset(&mut self) {
        self.next = Self::FIRST;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self { next: Self::FIRST }
    }
}

/// A probe that stayed up this long resets the backoff when it drops,
/// so a probe that dies right after starting keeps backing off.
pub const STABLE_AFTER: Duration = Duration::from_secs(30);

/// Sent by a host task to the monitor.
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    Probe(ConnState),
    Hello(HelloInfo),
    Hooks(Result<Vec<HookResult>, String>),
    Msg(ProbeMsg),
    /// A command could not be delivered (probe not connected).
    Dropped(AppMsg),
    /// Terminal connection state; `None` while no terminal is open.
    Term(Option<ConnState>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostCommand {
    /// Connect now: skips a backoff wait or leaves `NeedsUser`.
    Retry,
    Send(AppMsg),
    Stop,
}

type Events = UnboundedSender<(String, HostEvent)>;

struct Host<C> {
    cfg: HostConfig,
    connector: Arc<C>,
    events: Events,
    cmds: UnboundedReceiver<HostCommand>,
}

/// What a command received while waiting means for the wait.
enum Wake {
    Retry,
    Stop,
}

impl<C: Connector> Host<C> {
    fn emit(&self, ev: HostEvent) {
        let _ = self.events.send((self.cfg.id.clone(), ev));
    }

    /// Handle a command that arrived while no probe is running.
    fn idle_command(&self, cmd: Option<HostCommand>) -> Option<Wake> {
        match cmd {
            None | Some(HostCommand::Stop) => Some(Wake::Stop),
            Some(HostCommand::Retry) => Some(Wake::Retry),
            Some(HostCommand::Send(m)) => {
                self.emit(HostEvent::Dropped(m));
                None
            }
        }
    }

    /// Run `fut` while answering commands; `None` if told to stop.
    /// `Retry` is ignored: an attempt is already under way.
    async fn busy<F: Future>(&mut self, fut: F) -> Option<F::Output> {
        tokio::pin!(fut);
        loop {
            tokio::select! {
                out = &mut fut => return Some(out),
                cmd = self.cmds.recv() => {
                    if let Some(Wake::Stop) = self.idle_command(cmd) {
                        return None;
                    }
                }
            }
        }
    }

    /// Wait `delay` (forever if `None`) or until `Retry`; false on stop.
    async fn wait(&mut self, delay: Option<Duration>) -> bool {
        // Without a delay the sleep branch is disabled; its length is moot.
        let sleep = tokio::time::sleep(delay.unwrap_or(Duration::ZERO));
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep, if delay.is_some() => return true,
                cmd = self.cmds.recv() => match self.idle_command(cmd) {
                    Some(Wake::Retry) => return true,
                    Some(Wake::Stop) => return false,
                    None => {}
                },
            }
        }
    }

    /// Relay messages until the link fails (`Some(reason)`) or the task
    /// is told to stop (`None`). Hook events are acknowledged once they
    /// have been handed to the monitor.
    async fn serve(&mut self, link: &mut ProbeLink) -> Option<LinkError> {
        loop {
            tokio::select! {
                msg = link.recv() => {
                    let msg = match msg {
                        Ok(m) => m,
                        Err(e) => return Some(e),
                    };
                    let ack = match &msg {
                        ProbeMsg::AgentEvent(ev) => ev.spool_offset,
                        _ => None,
                    };
                    self.emit(HostEvent::Msg(msg));
                    if let Some(spool_offset) = ack
                        && let Err(e) = link.send(&AppMsg::Ack { spool_offset }).await
                    {
                        return Some(e);
                    }
                }
                cmd = self.cmds.recv() => match cmd {
                    None | Some(HostCommand::Stop) => return None,
                    Some(HostCommand::Retry) => {}
                    Some(HostCommand::Send(m)) => {
                        if let Err(e) = link.send(&m).await {
                            self.emit(HostEvent::Dropped(m));
                            return Some(e);
                        }
                    }
                },
            }
        }
    }

    async fn run(mut self) {
        let mut backoff = Backoff::default();
        loop {
            self.emit(HostEvent::Probe(ConnState::Connecting));
            let connector = self.connector.clone();
            let cfg = self.cfg.clone();
            let Some(opened) = self.busy(connector.open(&cfg)).await else {
                return;
            };
            let reason = match opened {
                Err(OpenError::NeedsUser(p)) => {
                    self.emit(HostEvent::Probe(ConnState::NeedsUser(p)));
                    if !self.wait(None).await {
                        return;
                    }
                    backoff.reset();
                    continue;
                }
                Err(OpenError::Retry(reason)) => reason,
                Ok(opened) => {
                    let mut link = ProbeLink::new(opened.io);
                    let Some(hello) = self.busy(link.hello()).await else {
                        return;
                    };
                    match hello {
                        Err(LinkError::Protocol { probe, app }) => {
                            let p = Problem::Protocol { probe, app };
                            self.emit(HostEvent::Probe(ConnState::NeedsUser(p)));
                            if !self.wait(None).await {
                                return;
                            }
                            continue;
                        }
                        Err(e) => with_stderr(e.to_string(), &opened.stderr).await,
                        Ok(hello) => {
                            self.emit(HostEvent::Hello(hello));
                            self.emit(HostEvent::Hooks(opened.hooks));
                            self.emit(HostEvent::Probe(ConnState::Up));
                            let up_since = Instant::now();
                            let Some(e) = self.serve(&mut link).await else {
                                return;
                            };
                            if up_since.elapsed() >= STABLE_AFTER {
                                backoff.reset();
                            }
                            with_stderr(e.to_string(), &opened.stderr).await
                        }
                    }
                }
            };
            let retry_in = backoff.next_delay();
            self.emit(HostEvent::Probe(ConnState::Retrying { retry_in, reason }));
            if !self.wait(Some(retry_in)).await {
                return;
            }
        }
    }
}

/// Run the probe connection of `cfg` until `HostCommand::Stop` or until
/// the command channel closes.
pub async fn run_host<C: Connector>(
    cfg: HostConfig,
    connector: Arc<C>,
    events: Events,
    cmds: UnboundedReceiver<HostCommand>,
) {
    Host {
        cfg,
        connector,
        events,
        cmds,
    }
    .run()
    .await
}
```

- [ ] **Step 7: `crates/mai-core/src/connect.rs`（完整内容）**

```rust
//! The real `Connector`: SSH hosts get the probe deployed over SSH and
//! `serve` started on an exec channel; the local host gets the probe
//! copied into the home directory and `serve` started as a child process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::BufReader;

use crate::deploy::{
    DeployError, DeployOptions, HookResult, ProbeStore, Remote, Shell, deploy, detect,
    hooks_result, normalize_arch, serve_argv, sha256_hex, stop_argv,
};
use crate::host::{Connector, HostConfig, HostKind, OpenError, Opened, Problem};
use crate::link::ProbeIo;
use crate::ssh::auth::{Prompter, SecretStore};
use crate::ssh::client::{ConnectOptions, ExecOutput, SshError, connect};
use crate::ssh::config::{HostSpec, parse_config, resolve};
use crate::stderr::{StderrTail, collect as collect_stderr};
use crate::terminals::SystemTerminals;

/// Locale requested for `serve` so zellij output is UTF-8.
const LOCALE: &str = "en_US.UTF-8";

/// Map an SSH failure to retry-or-ask-the-user.
pub fn ssh_open_error(e: SshError) -> OpenError {
    match e {
        SshError::Connect(m) | SshError::Channel(m) => OpenError::Retry(m),
        SshError::Auth(m) => OpenError::NeedsUser(Problem::Auth(m)),
        SshError::HostKeyRejected { .. } => OpenError::NeedsUser(Problem::HostKeyRejected),
        SshError::HostKeyChanged { file, line, .. } => {
            OpenError::NeedsUser(Problem::HostKeyChanged { file, line })
        }
        SshError::HostKeyRevoked { file, line, .. } => {
            OpenError::NeedsUser(Problem::HostKeyRevoked { file, line })
        }
    }
}

/// Map a deployment failure to retry-or-ask-the-user.
pub fn deploy_open_error(e: DeployError) -> OpenError {
    match e {
        DeployError::Ssh(e) => ssh_open_error(e),
        other => OpenError::NeedsUser(Problem::Deploy(other.to_string())),
    }
}

fn hooks_outcome(out: &ExecOutput) -> Result<Vec<HookResult>, String> {
    hooks_result(out).map_err(|e| e.to_string())
}

/// Remote-style description of this machine, for choosing the probe
/// binary and building command lines.
pub fn local_remote(home: &Path) -> Option<Remote> {
    use crate::deploy::Os;
    let os = match std::env::consts::OS {
        "linux" => Os::Linux,
        "macos" => Os::MacOs,
        "windows" => Os::Windows,
        _ => return None,
    };
    Some(Remote {
        os,
        arch: normalize_arch(std::env::consts::ARCH)?,
        shell: if os == Os::Windows {
            Shell::Cmd
        } else {
            Shell::Posix
        },
        home: home.to_string_lossy().into_owned(),
    })
}

/// Put `bytes` at `exe` unless it already has them. `stop` runs first
/// when an existing, different binary is replaced (a running probe locks
/// its file on Windows). Returns whether the file was written.
pub fn place_binary(exe: &Path, bytes: &[u8], stop: impl FnOnce()) -> std::io::Result<bool> {
    let existing = std::fs::read(exe).ok();
    if existing.as_deref().map(sha256_hex) == Some(sha256_hex(bytes)) {
        return Ok(false);
    }
    if let Some(dir) = exe.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = exe.as_os_str().to_owned();
    tmp.push(".upload");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    if existing.is_some() {
        stop();
        std::fs::remove_file(exe)?;
    }
    std::fs::rename(&tmp, exe)?;
    Ok(true)
}

#[cfg(windows)]
fn no_window(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    cmd.creation_flags(0x0800_0000)
}

#[cfg(not(windows))]
fn no_window(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    cmd
}

/// Run `exe` with `argv` and collect its output.
async fn run_local(exe: &Path, argv: &[String]) -> std::io::Result<ExecOutput> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(argv).stdin(Stdio::null());
    let out = no_window(&mut cmd).output().await?;
    Ok(ExecOutput {
        status: out.status.code().and_then(|c| u32::try_from(c).ok()),
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// Everything needed to reach hosts and start their probes.
pub struct SystemConnector<P, S> {
    pub opts: ConnectOptions,
    pub prompter: Arc<P>,
    pub secrets: Arc<S>,
    pub probes: ProbeStore,
    /// `~/.ssh/config`; read on every connect so edits take effect.
    pub ssh_config: Option<PathBuf>,
    /// Local home: default keys, and where the local probe is installed.
    pub home: PathBuf,
    /// User for targets that name none.
    pub default_user: String,
    /// This app installation's id (see `mai-probe serve --client`).
    pub client: String,
    /// Deploy dir under the home directory (`.mai`).
    pub dir: String,
    /// Install agent hooks after deploying.
    pub install_hooks: bool,
    gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl<P: Prompter, S: SecretStore> SystemConnector<P, S> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        opts: ConnectOptions,
        prompter: Arc<P>,
        secrets: Arc<S>,
        probes: ProbeStore,
        ssh_config: Option<PathBuf>,
        home: PathBuf,
        default_user: String,
        client: String,
    ) -> Self {
        Self {
            opts,
            prompter,
            secrets,
            probes,
            ssh_config,
            home,
            default_user,
            client,
            dir: ".mai".to_owned(),
            install_hooks: true,
            gates: Mutex::new(HashMap::new()),
        }
    }

    /// Lock held while connecting to and deploying on host `id`, so
    /// host-key prompts and uploads for one host never overlap.
    pub fn gate(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(|p| p.into_inner());
        gates.entry(id.to_owned()).or_default().clone()
    }

    fn spec(&self, target: &str) -> Result<HostSpec, OpenError> {
        let text = match &self.ssh_config {
            None => String::new(),
            Some(p) => match std::fs::read_to_string(p) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    return Err(OpenError::NeedsUser(Problem::Config(format!(
                        "{}: {e}",
                        p.display()
                    ))));
                }
            },
        };
        let config = parse_config(&text)
            .map_err(|e| OpenError::NeedsUser(Problem::Config(e.to_string())))?;
        resolve(&config, target, &self.default_user, &self.home)
            .map_err(|e| OpenError::NeedsUser(Problem::Config(e.to_string())))
    }

    async fn open_ssh(&self, host: &HostConfig, target: &str) -> Result<Opened, OpenError> {
        let spec = self.spec(target)?;
        let gate = self.gate(&host.id);
        let guard = gate.lock().await;
        let session = connect(&spec, &self.opts, self.prompter.clone(), &*self.secrets)
            .await
            .map_err(ssh_open_error)?;
        let opts = DeployOptions {
            dir: self.dir.clone(),
            install_hooks: false,
            client: self.client.clone(),
        };
        let report = deploy(&session, &self.probes, &opts)
            .await
            .map_err(deploy_open_error)?;
        let remote = &report.remote;
        let hooks = if self.install_hooks {
            let cmd = remote.invoke(&report.probe_path, "install-hooks");
            let out = session.exec(&cmd).await.map_err(ssh_open_error)?;
            hooks_outcome(&out)
        } else {
            Ok(Vec::new())
        };
        drop(guard);
        let args = remote.serve_args(&self.client, host.zellij.as_deref());
        let env = [("LANG", LOCALE), ("LC_CTYPE", LOCALE)];
        let stderr = Arc::new(StderrTail::default());
        let (r, w) = session
            .open_exec_split(
                &remote.invoke(&report.probe_path, &args),
                &env,
                stderr.clone(),
            )
            .await
            .map_err(ssh_open_error)?;
        Ok(Opened {
            io: ProbeIo {
                reader: Box::new(BufReader::new(r)),
                writer: w,
            },
            hooks,
            keep: Box::new(session),
            stderr,
        })
    }

    async fn open_local(&self, host: &HostConfig) -> Result<Opened, OpenError> {
        let deploy_err = |m: String| OpenError::NeedsUser(Problem::Deploy(m));
        let remote = local_remote(&self.home)
            .ok_or_else(|| deploy_err("unsupported local platform".into()))?;
        let target = remote
            .target()
            .ok_or_else(|| deploy_err("unsupported local platform".into()))?;
        let bin = self.probes.binary(target);
        let bytes =
            std::fs::read(&bin).map_err(|e| deploy_err(format!("{}: {e}", bin.display())))?;
        let exe = PathBuf::from(remote.probe_path(&self.dir));
        let gate = self.gate(&host.id);
        let guard = gate.lock().await;
        let (place_exe, stop) = (exe.clone(), stop_argv(&self.client));
        let placed = tokio::task::spawn_blocking(move || {
            place_binary(&place_exe, &bytes, || {
                let mut cmd = std::process::Command::new(&place_exe);
                cmd.args(&stop).stdin(Stdio::null()).stdout(Stdio::null());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x0800_0000);
                }
                let _ = cmd.status();
            })
        })
        .await
        .map_err(|e| deploy_err(e.to_string()))?;
        placed.map_err(|e| deploy_err(format!("{}: {e}", exe.display())))?;
        let hooks = if self.install_hooks {
            match run_local(&exe, &["install-hooks".to_owned()]).await {
                Ok(out) => hooks_outcome(&out),
                Err(e) => Err(e.to_string()),
            }
        } else {
            Ok(Vec::new())
        };
        drop(guard);
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.args(serve_argv(&self.client, host.zellij.as_deref()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = no_window(&mut cmd)
            .spawn()
            .map_err(|e| OpenError::Retry(format!("start {}: {e}", exe.display())))?;
        let (Some(out), Some(input), Some(err)) =
            (child.stdout.take(), child.stdin.take(), child.stderr.take())
        else {
            return Err(OpenError::Retry("probe pipes unavailable".into()));
        };
        let stderr = Arc::new(StderrTail::default());
        tokio::spawn(collect_stderr(err, stderr.clone()));
        Ok(Opened {
            io: ProbeIo {
                reader: Box::new(BufReader::new(out)),
                writer: Box::new(input),
            },
            hooks,
            keep: Box::new(child),
            stderr,
        })
    }
}

impl<P: Prompter, S: SecretStore> SystemConnector<P, S> {
    /// A second SSH session for terminals. Connecting holds the host's
    /// gate, so it never prompts for a host key at the same time as the
    /// probe connection.
    async fn open_ssh_terminals(
        &self,
        host: &HostConfig,
        target: &str,
    ) -> Result<SystemTerminals<P>, OpenError> {
        let spec = self.spec(target)?;
        let gate = self.gate(&host.id);
        let _guard = gate.lock().await;
        let session = connect(&spec, &self.opts, self.prompter.clone(), &*self.secrets)
            .await
            .map_err(ssh_open_error)?;
        let remote = detect(&session).await.map_err(deploy_open_error)?;
        Ok(SystemTerminals::Ssh { session, remote })
    }
}

impl<P: Prompter, S: SecretStore> Connector for SystemConnector<P, S> {
    type Terminals = SystemTerminals<P>;

    async fn open(&self, host: &HostConfig) -> Result<Opened, OpenError> {
        match &host.kind {
            HostKind::Local => self.open_local(host).await,
            HostKind::Ssh { target } => self.open_ssh(host, target).await,
        }
    }

    async fn open_terminals(&self, host: &HostConfig) -> Result<SystemTerminals<P>, OpenError> {
        match &host.kind {
            HostKind::Local => Ok(SystemTerminals::Local),
            HostKind::Ssh { target } => self.open_ssh_terminals(host, target).await,
        }
    }
}
```

- [ ] **Step 8: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: `hostkey` 13 个、`connect` 5 个测试通过，全部 224 个；clippy 无警告。

- [ ] **Step 9: 提交**

```bash
git add Cargo.lock crates/mai-core
git commit -m "feat(core): lenient known_hosts parser with hashed names and @revoked

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: ssh config 的 `Match` 块与跳板主机（B4、B14）

**Files:**

- Modify: `crates/mai-core/src/ssh/config.rs`
- Test: `crates/mai-core/tests/ssh_config.rs`

**Interfaces:**

- Produces:

  ```rust
  // ssh::config
  pub fn config_warnings(text: &str) -> Vec<String>
  // 每个 Match 行一条：
  // "ssh config line N: Match blocks are not supported; their settings are ignored"
  ```

  `parse_config` 先去掉 `Match` 块（到下一个 `Host` 或文件末尾）；`resolve` 为
  每个跳板主机只解析它自己的 HostName/User/Port/IdentityFile，跳板主机的
  `jumps` 恒为空；删除 `MAX_JUMP_DEPTH` 与“ProxyJump 循环”错误。

- [ ] **Step 1: 写失败测试 `crates/mai-core/tests/ssh_config.rs`（完整内容）**

```rust
use std::path::{Path, PathBuf};

use mai_core::ssh::config::{config_warnings, parse_config, resolve};

const CONFIG: &str = "\
Host mac
  HostName 100.96.237.7
  User cyt

Host box
  HostName box.internal
  Port 2222
  IdentityFile ~/.ssh/box_key
  ProxyJump mac
  ForwardX11Trusted yes

Host loop-a
  ProxyJump loop-b

Host loop-b
  ProxyJump loop-a

Host *
  User fallback
";

fn home() -> PathBuf {
    PathBuf::from("/home/me")
}

fn default_keys(home: &Path) -> Vec<PathBuf> {
    ["id_ed25519", "id_ecdsa", "id_rsa"]
        .iter()
        .map(|k| home.join(".ssh").join(k))
        .collect()
}

#[test]
fn alias_uses_config_values_and_default_keys() {
    let cfg = parse_config(CONFIG).unwrap();
    let spec = resolve(&cfg, "mac", "local", &home()).unwrap();
    assert_eq!(spec.alias, "mac");
    assert_eq!(spec.host, "100.96.237.7");
    assert_eq!(spec.port, 22);
    assert_eq!(spec.user, "cyt");
    assert_eq!(spec.identity_files, default_keys(&home()));
    assert!(spec.jumps.is_empty());
}

#[test]
fn port_identity_jump_and_wildcard_user() {
    let cfg = parse_config(CONFIG).unwrap();
    let spec = resolve(&cfg, "box", "local", &home()).unwrap();
    assert_eq!((spec.host.as_str(), spec.port), ("box.internal", 2222));
    assert_eq!(spec.user, "fallback");
    assert_eq!(spec.identity_files.len(), 1);
    assert!(
        spec.identity_files[0].is_absolute(),
        "{:?}",
        spec.identity_files
    );
    assert!(spec.identity_files[0].ends_with(Path::new(".ssh").join("box_key")));
    assert_eq!(spec.jumps.len(), 1);
    assert_eq!(spec.jumps[0].host, "100.96.237.7");
    assert_eq!(spec.jumps[0].user, "cyt");
}

#[test]
fn explicit_user_host_port() {
    let cfg = parse_config(CONFIG).unwrap();
    let spec = resolve(&cfg, "root@10.0.0.5:2200", "local", &home()).unwrap();
    assert_eq!(
        (spec.user.as_str(), spec.host.as_str(), spec.port),
        ("root", "10.0.0.5", 2200)
    );
    let spec = resolve(&cfg, "ops@mac", "local", &home()).unwrap();
    assert_eq!(
        (spec.user.as_str(), spec.host.as_str()),
        ("ops", "100.96.237.7")
    );
}

#[test]
fn ipv6_forms() {
    let cfg = parse_config("").unwrap();
    let spec = resolve(&cfg, "[fe80::1]:2022", "me", &home()).unwrap();
    assert_eq!((spec.host.as_str(), spec.port), ("fe80::1", 2022));
    let spec = resolve(&cfg, "fe80::1", "me", &home()).unwrap();
    assert_eq!((spec.host.as_str(), spec.port), ("fe80::1", 22));
}

#[test]
fn unknown_alias_falls_back_to_name_and_default_user() {
    let cfg = parse_config("").unwrap();
    let spec = resolve(&cfg, "plain.host", "me", &home()).unwrap();
    assert_eq!(
        (spec.host.as_str(), spec.user.as_str()),
        ("plain.host", "me")
    );
}

#[test]
fn jump_hosts_own_proxyjump_is_not_followed() {
    let cfg = parse_config(CONFIG).unwrap();
    let spec = resolve(&cfg, "loop-a", "me", &home()).unwrap();
    assert_eq!(spec.jumps.len(), 1);
    assert_eq!(spec.jumps[0].host, "loop-b");
    assert!(spec.jumps[0].jumps.is_empty());
}

const WITH_MATCH: &str = "\
Host a
  User alice
Match host a exec \"true\"
  User mallory
  Port 2222
Host b
  User bob
";

#[test]
fn match_blocks_are_ignored_not_misattributed() {
    let cfg = parse_config(WITH_MATCH).unwrap();
    let a = resolve(&cfg, "a", "me", &home()).unwrap();
    assert_eq!((a.user.as_str(), a.port), ("alice", 22));
    let b = resolve(&cfg, "b", "me", &home()).unwrap();
    assert_eq!(b.user, "bob");
}

#[test]
fn match_blocks_are_reported() {
    assert_eq!(
        config_warnings(WITH_MATCH),
        vec![
            "ssh config line 3: Match blocks are not supported; their settings are ignored"
                .to_owned()
        ]
    );
    assert!(config_warnings(CONFIG).is_empty());
}

#[test]
fn bad_port_is_an_error() {
    let cfg = parse_config("").unwrap();
    assert!(resolve(&cfg, "host:notaport", "me", &home()).is_err());
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test ssh_config`
Expected: 编译失败（`config_warnings` 不存在）。

- [ ] **Step 3: 写 `crates/mai-core/src/ssh/config.rs`（完整内容）**

```rust
//! Resolve a host alias or `[user@]host[:port]` into connection
//! parameters, using OpenSSH config text (`~/.ssh/config`).
//!
//! `Match` blocks are not supported: the parser would attribute their
//! directives to the preceding `Host`, so they are removed before parsing
//! and reported by `config_warnings`.

use std::fmt;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use ssh2_config::{ParseRule, SshConfig};

/// Everything needed to open one SSH connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSpec {
    /// What the user typed (alias or `[user@]host[:port]`).
    pub alias: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// Tried in order; missing files are skipped at auth time.
    pub identity_files: Vec<PathBuf>,
    /// Jump hosts, outermost first (`ProxyJump`). Their own `ProxyJump`
    /// settings are not followed, so their `jumps` are always empty.
    pub jumps: Vec<HostSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError(pub String);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ConfigError {}

const DEFAULT_PORT: u16 = 22;
const DEFAULT_KEYS: &[&str] = &["id_ed25519", "id_ecdsa", "id_rsa"];

/// First word of a config line, lowercased (`Host`, `Match`, ...).
fn keyword(line: &str) -> String {
    line.trim_start()
        .split(|c: char| c.is_whitespace() || c == '=')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Lines of `text` without `Match` blocks. A block runs from a `Match`
/// line to the next `Host` or `Match` line.
fn without_match_blocks(text: &str) -> String {
    let mut out = String::new();
    let mut in_match = false;
    for line in text.lines() {
        match keyword(line).as_str() {
            "match" => in_match = true,
            "host" => in_match = false,
            _ => {}
        }
        if !in_match {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Things in `text` that are ignored and the user should know about: one
/// entry per `Match` block, with its 1-based line number.
pub fn config_warnings(text: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| keyword(l) == "match")
        .map(|(i, _)| {
            format!(
                "ssh config line {}: Match blocks are not supported; their settings are ignored",
                i + 1
            )
        })
        .collect()
}

/// Parse OpenSSH config text; unknown directives and `Match` blocks are
/// ignored.
pub fn parse_config(text: &str) -> Result<SshConfig, ConfigError> {
    SshConfig::default()
        .parse(
            &mut BufReader::new(without_match_blocks(text).as_bytes()),
            ParseRule::ALLOW_UNKNOWN_FIELDS,
        )
        .map_err(|e| ConfigError(format!("ssh config: {e}")))
}

/// Split `[user@]host[:port]`. A bare IPv6 address (several `:`) is kept
/// whole unless written as `[addr]:port`.
fn split_target(target: &str) -> Result<(Option<&str>, &str, Option<u16>), ConfigError> {
    let (user, rest) = match target.rsplit_once('@') {
        Some((u, r)) => (Some(u), r),
        None => (None, target),
    };
    let bad_port = || ConfigError(format!("bad port in '{target}'"));
    if let Some(inner) = rest.strip_prefix('[') {
        let (addr, tail) = inner
            .split_once(']')
            .ok_or_else(|| ConfigError(format!("unclosed '[' in '{target}'")))?;
        let port = match tail.strip_prefix(':') {
            Some(p) => Some(p.parse().map_err(|_| bad_port())?),
            None => None,
        };
        return Ok((user, addr, port));
    }
    match rest.split_once(':') {
        Some((host, port)) if !port.contains(':') => {
            Ok((user, host, Some(port.parse().map_err(|_| bad_port())?)))
        }
        _ => Ok((user, rest, None)),
    }
}

/// Resolve `target` against `config`. `default_user` applies when neither
/// the target nor the config names a user; `home` locates default keys.
pub fn resolve(
    config: &SshConfig,
    target: &str,
    default_user: &str,
    home: &Path,
) -> Result<HostSpec, ConfigError> {
    let mut spec = resolve_hop(config, target, default_user, home)?;
    let params = config.query(split_target(target)?.1);
    spec.jumps = params
        .proxy_jump
        .clone()
        .unwrap_or_default()
        .iter()
        .flat_map(|j| j.split(','))
        .map(str::trim)
        .filter(|j| !j.is_empty() && !j.eq_ignore_ascii_case("none"))
        .map(|j| resolve_hop(config, j, default_user, home))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(spec)
}

/// One host's parameters, without jump hosts.
fn resolve_hop(
    config: &SshConfig,
    target: &str,
    default_user: &str,
    home: &Path,
) -> Result<HostSpec, ConfigError> {
    let (user, name, port) = split_target(target)?;
    let params = config.query(name);
    let identity_files = params.identity_file.clone().unwrap_or_else(|| {
        DEFAULT_KEYS
            .iter()
            .map(|k| home.join(".ssh").join(k))
            .collect()
    });
    Ok(HostSpec {
        alias: target.to_owned(),
        host: params.host_name.clone().unwrap_or_else(|| name.to_owned()),
        port: port.or(params.port).unwrap_or(DEFAULT_PORT),
        user: user
            .map(str::to_owned)
            .or(params.user.clone())
            .unwrap_or_else(|| default_user.to_owned()),
        identity_files,
        jumps: Vec::new(),
    })
}
```

- [ ] **Step 4: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: `ssh_config` 9 个测试通过，全部 226 个；clippy 无警告。

- [ ] **Step 5: 提交**

```bash
git add crates/mai-core
git commit -m "feat(core): ignore ssh config Match blocks with a warning; do not follow nested ProxyJump

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: 认证加固（B9、B16、B7、B5、B13）

**Files:**

- Create: `crates/mai-core/src/ssh/signer.rs`
- Modify: `crates/mai-core/src/ssh/mod.rs`、`crates/mai-core/src/ssh/auth.rs`、
  `crates/mai-core/src/ssh/client.rs`
- Test: `crates/mai-core/tests/ssh_session.rs`

**Interfaces:**

- Consumes: Task 1 的 `client.rs`。
- Produces:

  ```rust
  // ssh::signer
  pub fn public_key_of(path: &Path) -> Option<PublicKey>   // <file>.pub，否则未加密私钥
  pub async fn load_key<P: Prompter, S: SecretStore>(path: &Path, prompter: &P,
      secrets: &S, notes: &mut Vec<String>) -> Option<PrivateKey>
  pub fn signed(key: &PrivateKey, hash_alg: Option<HashAlg>, to_sign: &[u8]) -> Option<Vec<u8>>
  pub fn bogus_signed(public: &PublicKey, hash_alg: Option<HashAlg>, to_sign: &[u8]) -> Vec<u8>
  pub struct SignError;                                     // From<russh::SendError>
  pub struct FileSigner<'a, P, S> { pub path: &'a Path, pub prompter: &'a P,
      pub secrets: &'a S, pub notes: &'a mut Vec<String> } // impl russh::Signer
  // ssh::client
  pub const MAX_KBD_ROUNDS: usize = 10;
  impl SshSession<P> { pub fn notes(&self) -> &[String] }   // 各跳的提示合并
  // ssh::auth
  pub fn passphrase_key(key_file: &Path) -> String          // canonicalize；Windows 去 \\?\ 并小写
  ```

  `FileSigner::auth_sign` 只在服务器回 PK_OK 后被调用；用户不给口令时用
  `bogus_signed` 返回一个必然被拒绝的签名（空签名会让服务器断开会话），认证随后
  继续尝试其他方法。

- [ ] **Step 1: 写失败测试 `crates/mai-core/tests/ssh_session.rs`（完整内容）**

测试服务器新增：按公钥判断是否回 PK_OK（`auth_publickey_offered`）、
`direct-tcpip` 转发（用于跳板机测试）、回答 `again` 时无限追问的
keyboard-interactive。

```rust
//! SSH client tests against an in-process russh server (no network, no
//! external sshd). Keys are generated per test run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mai_core::ssh::auth::{KbdPrompt, Prompter, Secret, SecretStore, passphrase_key, password_key};
use mai_core::ssh::client::{ConnectOptions, SshError, connect};
use mai_core::ssh::config::HostSpec;
use russh::keys::{Algorithm, PrivateKey, PublicKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use ssh_key::LineEnding;
use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;

const PASSWORD: &str = "s3cret";
const OTP: &str = "123456";

fn random_key() -> PrivateKey {
    PrivateKey::random(&mut UnwrapErr(SysRng), Algorithm::Ed25519).unwrap()
}

/// What the test server accepts.
#[derive(Clone)]
struct ServerCfg {
    methods: Vec<MethodKind>,
    client_key: Option<PublicKey>,
}

#[derive(Clone)]
struct TestServer(ServerCfg);

impl TestServer {
    /// Reject but keep every method available, like OpenSSH (which allows
    /// several attempts); russh's default reject drops the method.
    fn retry(&self) -> Auth {
        Auth::Reject {
            proceed_with_methods: Some(MethodSet::from(&self.0.methods[..])),
            partial_success: false,
        }
    }
}

impl server::Handler for TestServer {
    type Error = russh::Error;

    async fn auth_password(&mut self, _user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if password == PASSWORD {
            Auth::Accept
        } else {
            self.retry()
        })
    }

    /// Like OpenSSH: only an authorized key gets `PK_OK` for a probe.
    async fn auth_publickey_offered(
        &mut self,
        user: &str,
        key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        self.auth_publickey(user, key).await
    }

    async fn auth_publickey(&mut self, _user: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        let ok = self
            .0
            .client_key
            .as_ref()
            .is_some_and(|k| k.key_data() == key.key_data());
        Ok(if ok { Auth::Accept } else { self.retry() })
    }

    async fn auth_keyboard_interactive<'a>(
        &'a mut self,
        _user: &str,
        _submethods: &str,
        response: Option<server::Response<'a>>,
    ) -> Result<Auth, Self::Error> {
        let Some(mut response) = response else {
            return Ok(Auth::Partial {
                name: "otp".into(),
                instructions: "enter code".into(),
                prompts: vec![("Code: ".into(), false)].into(),
            });
        };
        let answer = response
            .next()
            .map(|b| String::from_utf8_lossy(&b).into_owned());
        // "again" makes the server ask another round, forever.
        if answer.as_deref() == Some("again") {
            return Ok(Auth::Partial {
                name: "otp".into(),
                instructions: "enter code".into(),
                prompts: vec![("Code: ".into(), false)].into(),
            });
        }
        Ok(if answer.as_deref() == Some(OTP) {
            Auth::Accept
        } else {
            self.retry()
        })
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    /// Forward a `direct-tcpip` channel to a local TCP port (a jump host).
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<Msg>,
        host: &str,
        port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let addr = format!("{host}:{port}");
        reply.accept().await;
        tokio::spawn(async move {
            if let Ok(mut tcp) = tokio::net::TcpStream::connect(addr).await {
                let mut stream = channel.into_stream();
                let _ = tokio::io::copy_bidirectional(&mut stream, &mut tcp).await;
            }
        });
        Ok(())
    }

    /// Echo the command on stdout, "e" on stderr, exit status 7.
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, data.to_vec())?;
        session.extended_data(channel, 1, b"e".to_vec())?;
        session.exit_status_request(channel, 7)?;
        session.eof(channel)?;
        session.close(channel)?;
        Ok(())
    }
}

/// Start a server on 127.0.0.1:<random>; returns its port and host key.
async fn start(cfg: ServerCfg) -> (u16, PublicKey) {
    start_after(cfg, Duration::ZERO).await
}

/// Like `start`, but each accepted connection waits `delay` before the SSH
/// handshake (a slow server).
async fn start_after(cfg: ServerCfg, delay: Duration) -> (u16, PublicKey) {
    let host_key = random_key();
    let public = host_key.public_key().clone();
    let config = Arc::new(server::Config {
        keys: vec![host_key],
        methods: MethodSet::from(&cfg.methods[..]),
        auth_rejection_time: Duration::from_millis(1),
        auth_rejection_time_initial: Some(Duration::from_millis(1)),
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (config, cfg) = (config.clone(), cfg.clone());
            tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let _ = server::run_stream(config, stream, TestServer(cfg)).await;
            });
        }
    });
    (port, public)
}

#[derive(Default)]
struct Scripted {
    trust: bool,
    /// How long the user "thinks" before answering the host-key prompt.
    confirm_delay: Duration,
    password: Option<Secret>,
    passphrase: Option<Secret>,
    kbd: Option<Vec<String>>,
    calls: Mutex<Vec<String>>,
}

impl Scripted {
    fn log(&self, what: &str) {
        self.calls.lock().unwrap().push(what.to_owned());
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl Prompter for Scripted {
    fn confirm_host_key(&self, _h: &str, _p: u16, _fp: &str) -> impl Future<Output = bool> + Send {
        self.log("host_key");
        let (t, delay) = (self.trust, self.confirm_delay);
        async move {
            tokio::time::sleep(delay).await;
            t
        }
    }
    fn password(&self, _u: &str, _h: &str) -> impl Future<Output = Option<Secret>> + Send {
        self.log("password");
        let v = self.password.clone();
        async move { v }
    }
    fn passphrase(&self, _k: &Path) -> impl Future<Output = Option<Secret>> + Send {
        self.log("passphrase");
        let v = self.passphrase.clone();
        async move { v }
    }
    fn keyboard_interactive(
        &self,
        _n: &str,
        _i: &str,
        prompts: &[KbdPrompt],
    ) -> impl Future<Output = Option<Vec<String>>> + Send {
        self.log(&format!("kbd:{}", prompts.len()));
        let v = self.kbd.clone();
        async move { v }
    }
}

#[derive(Default)]
struct MemStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemStore {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().unwrap().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

fn spec(port: u16, identity_files: Vec<PathBuf>) -> HostSpec {
    HostSpec {
        alias: "test".into(),
        host: "127.0.0.1".into(),
        port,
        user: "tester".into(),
        identity_files,
        jumps: vec![],
    }
}

fn opts(dir: &Path) -> ConnectOptions {
    let learn_to = dir.join("known_hosts");
    ConnectOptions {
        known_hosts: vec![learn_to.clone()],
        learn_to,
        timeout: Duration::from_secs(10),
    }
}

fn secret(v: &str, remember: bool) -> Option<Secret> {
    Some(Secret {
        value: v.into(),
        remember,
    })
}

#[tokio::test]
async fn prompted_password_is_remembered_and_host_key_learned() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, true),
        ..Default::default()
    });
    let store = MemStore::default();
    let s = connect(&spec(port, vec![]), &opts(dir.path()), p.clone(), &store)
        .await
        .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "password"]);
    assert_eq!(store.get(&password_key("test")).as_deref(), Some(PASSWORD));
    assert!(
        std::fs::read_to_string(dir.path().join("known_hosts"))
            .unwrap()
            .contains("127.0.0.1")
    );
    s.close().await;

    // Second connection: key known, password from the store, no prompts.
    let p2 = Arc::new(Scripted::default());
    connect(&spec(port, vec![]), &opts(dir.path()), p2.clone(), &store)
        .await
        .unwrap();
    assert!(p2.calls().is_empty(), "{:?}", p2.calls());
}

/// `learn_to` need not be listed in `known_hosts` for a key already
/// learned there to be found again: the checker always consults it too.
#[tokio::test]
async fn learn_to_is_checked_even_if_absent_from_known_hosts() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let o = ConnectOptions {
        known_hosts: vec![dir.path().join("does_not_exist")],
        learn_to: dir.path().join("app_known_hosts"),
        timeout: Duration::from_secs(10),
    };
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    connect(&spec(port, vec![]), &o, p.clone(), &MemStore::default())
        .await
        .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "password"]);

    // Second connection: `known_hosts` still does not list `learn_to`, but
    // the key was written there, so it must be found without a prompt.
    let p2 = Arc::new(Scripted {
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    connect(&spec(port, vec![]), &o, p2.clone(), &MemStore::default())
        .await
        .unwrap();
    assert_eq!(p2.calls(), vec!["password"]);
}

#[tokio::test]
async fn wrong_stored_password_is_dropped_then_prompted() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let store = MemStore::default();
    store.set(&password_key("test"), "stale").unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    connect(&spec(port, vec![]), &opts(dir.path()), p.clone(), &store)
        .await
        .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "password"]);
    assert_eq!(store.get(&password_key("test")), None);
}

#[tokio::test]
async fn keyboard_interactive_answers_prompts() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::KeyboardInteractive],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        kbd: Some(vec![OTP.into()]),
        ..Default::default()
    });
    connect(
        &spec(port, vec![]),
        &opts(dir.path()),
        p.clone(),
        &MemStore::default(),
    )
    .await
    .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "kbd:1"]);
}

#[tokio::test]
async fn private_key_file_needs_no_prompts() {
    let client = random_key();
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::PublicKey],
        client_key: Some(client.public_key().clone()),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("id_test");
    client
        .write_openssh_file(&key_file, LineEnding::LF)
        .unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        ..Default::default()
    });
    let files = vec![dir.path().join("missing_key"), key_file];
    connect(
        &spec(port, files),
        &opts(dir.path()),
        p.clone(),
        &MemStore::default(),
    )
    .await
    .unwrap();
    assert_eq!(p.calls(), vec!["host_key"]);
}

#[tokio::test]
async fn encrypted_key_asks_passphrase_once_and_remembers() {
    let client = random_key();
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::PublicKey],
        client_key: Some(client.public_key().clone()),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("id_enc");
    client
        .encrypt(&mut SysRng, "pw")
        .unwrap()
        .write_openssh_file(&key_file, LineEnding::LF)
        .unwrap();
    let store = MemStore::default();
    let p = Arc::new(Scripted {
        trust: true,
        passphrase: secret("pw", true),
        ..Default::default()
    });
    connect(
        &spec(port, vec![key_file.clone()]),
        &opts(dir.path()),
        p.clone(),
        &store,
    )
    .await
    .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "passphrase"]);
    assert_eq!(store.get(&passphrase_key(&key_file)).as_deref(), Some("pw"));
}

#[tokio::test]
async fn declined_host_key_aborts() {
    let (port, public) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: false,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    let err = connect(
        &spec(port, vec![]),
        &opts(dir.path()),
        p.clone(),
        &MemStore::default(),
    )
    .await
    .err()
    .unwrap();
    let SshError::HostKeyRejected { fingerprint, .. } = err else {
        panic!("unexpected {err:?}");
    };
    assert_eq!(fingerprint, mai_core::ssh::hostkey::fingerprint(&public));
    assert_eq!(p.calls(), vec!["host_key"]);
}

#[tokio::test]
async fn changed_host_key_is_refused_without_prompt() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let other = random_key();
    mai_core::ssh::hostkey::learn(
        &dir.path().join("known_hosts"),
        "127.0.0.1",
        port,
        other.public_key(),
    )
    .unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        ..Default::default()
    });
    let err = connect(
        &spec(port, vec![]),
        &opts(dir.path()),
        p.clone(),
        &MemStore::default(),
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(err, SshError::HostKeyChanged { .. }), "{err:?}");
    assert!(p.calls().is_empty());
}

#[tokio::test]
async fn all_methods_failing_is_an_auth_error() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        password: secret("nope", true),
        ..Default::default()
    });
    let store = MemStore::default();
    let err = connect(&spec(port, vec![]), &opts(dir.path()), p, &store)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, SshError::Auth(_)), "{err:?}");
    assert_eq!(
        store.get(&password_key("test")),
        None,
        "wrong password not remembered"
    );
}

#[tokio::test]
async fn exec_collects_stdout_stderr_and_status() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    let s = connect(
        &spec(port, vec![]),
        &opts(dir.path()),
        p,
        &MemStore::default(),
    )
    .await
    .unwrap();
    let out = s.exec("uname -sm").await.unwrap();
    assert_eq!(out.stdout_str(), "uname -sm");
    assert_eq!(out.stderr, b"e");
    assert_eq!(out.status, Some(7));
    assert!(!out.success());
}

#[tokio::test]
async fn unreachable_host_is_a_connect_error() {
    let dir = tempfile::tempdir().unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let p = Arc::new(Scripted::default());
    let err = connect(
        &spec(port, vec![]),
        &opts(dir.path()),
        p,
        &MemStore::default(),
    )
    .await
    .err()
    .unwrap();
    assert!(matches!(err, SshError::Connect(_)), "{err:?}");
}

/// The connect timeout must not count the time the user spends on the
/// host-key prompt.
#[tokio::test]
async fn slow_host_key_answer_does_not_time_out() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut o = opts(dir.path());
    o.timeout = Duration::from_millis(300);
    let p = Arc::new(Scripted {
        trust: true,
        confirm_delay: Duration::from_millis(900),
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    connect(&spec(port, vec![]), &o, p.clone(), &MemStore::default())
        .await
        .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "password"]);
}

/// A connection that timed out before the host-key prompt must never
/// prompt or record the key later (the russh session task is detached and
/// keeps running after `connect` gives up).
#[tokio::test]
async fn timed_out_connection_never_prompts_later() {
    let (port, _) = start_after(
        ServerCfg {
            methods: vec![MethodKind::Password],
            client_key: None,
        },
        Duration::from_millis(600),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let mut o = opts(dir.path());
    o.timeout = Duration::from_millis(200);
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    let err = connect(&spec(port, vec![]), &o, p.clone(), &MemStore::default())
        .await
        .err()
        .unwrap();
    assert!(
        matches!(err, SshError::Connect(ref m) if m.contains("timed out")),
        "{err:?}"
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(p.calls().is_empty(), "{:?}", p.calls());
    assert!(!dir.path().join("known_hosts").exists());
}

#[tokio::test]
async fn rejected_encrypted_key_is_never_decrypted() {
    let accepted = random_key();
    let rejected = random_key();
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::PublicKey],
        client_key: Some(accepted.public_key().clone()),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let write = |k: &PrivateKey, name: &str| {
        let f = dir.path().join(name);
        k.encrypt(&mut SysRng, "pw")
            .unwrap()
            .write_openssh_file(&f, LineEnding::LF)
            .unwrap();
        f
    };
    let files = vec![write(&rejected, "id_other"), write(&accepted, "id_ok")];
    let p = Arc::new(Scripted {
        trust: true,
        passphrase: secret("pw", false),
        ..Default::default()
    });
    connect(
        &spec(port, files),
        &opts(dir.path()),
        p.clone(),
        &MemStore::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        p.calls(),
        vec!["host_key", "passphrase"],
        "only the accepted key"
    );
}

#[tokio::test]
async fn refused_passphrase_moves_on_to_password() {
    let client = random_key();
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::PublicKey, MethodKind::Password],
        client_key: Some(client.public_key().clone()),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("id_enc");
    client
        .encrypt(&mut SysRng, "pw")
        .unwrap()
        .write_openssh_file(&key_file, LineEnding::LF)
        .unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        passphrase: None,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    connect(
        &spec(port, vec![key_file]),
        &opts(dir.path()),
        p.clone(),
        &MemStore::default(),
    )
    .await
    .unwrap();
    assert_eq!(p.calls(), vec!["host_key", "passphrase", "password"]);
}

#[tokio::test]
async fn endless_keyboard_interactive_gives_up() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::KeyboardInteractive],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        kbd: Some(vec!["again".into()]),
        ..Default::default()
    });
    let err = connect(
        &spec(port, vec![]),
        &opts(dir.path()),
        p.clone(),
        &MemStore::default(),
    )
    .await
    .err()
    .expect("auth fails");
    assert!(matches!(err, SshError::Auth(_)), "{err:?}");
    let rounds = p.calls().iter().filter(|c| c.starts_with("kbd")).count();
    assert_eq!(rounds, mai_core::ssh::client::MAX_KBD_ROUNDS);
}

/// A keychain that refuses every write.
struct ReadOnlyStore;

impl SecretStore for ReadOnlyStore {
    fn get(&self, _key: &str) -> Option<String> {
        None
    }
    fn set(&self, _key: &str, _value: &str) -> Result<(), String> {
        Err("keychain locked".into())
    }
    fn delete(&self, _key: &str) -> Result<(), String> {
        Err("keychain locked".into())
    }
}

#[tokio::test]
async fn keychain_failure_is_reported_not_swallowed() {
    let (port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, true),
        ..Default::default()
    });
    let s = connect(&spec(port, vec![]), &opts(dir.path()), p, &ReadOnlyStore)
        .await
        .unwrap();
    assert_eq!(s.notes().len(), 1, "{:?}", s.notes());
    assert!(
        s.notes()[0].contains("could not save the password"),
        "{:?}",
        s.notes()
    );
    assert!(s.notes()[0].contains("keychain locked"), "{:?}", s.notes());
}

#[test]
fn passphrase_key_is_the_same_for_every_spelling_of_a_path() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let file = dir.path().join("id_key");
    std::fs::write(&file, "x").unwrap();
    let detour = dir.path().join("sub").join("..").join("id_key");
    assert_eq!(passphrase_key(&file), passphrase_key(&detour));
    if cfg!(windows) {
        let upper = PathBuf::from(file.to_string_lossy().to_uppercase());
        assert_eq!(passphrase_key(&file), passphrase_key(&upper));
    }
    let missing = dir.path().join("nope");
    assert!(passphrase_key(&missing).starts_with("passphrase:"));
}

#[tokio::test]
async fn connects_through_a_jump_host() {
    let (jump_port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let (target_port, _) = start(ServerCfg {
        methods: vec![MethodKind::Password],
        client_key: None,
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let p = Arc::new(Scripted {
        trust: true,
        password: secret(PASSWORD, false),
        ..Default::default()
    });
    let mut target = spec(target_port, vec![]);
    target.jumps = vec![HostSpec {
        alias: "jump".into(),
        ..spec(jump_port, vec![])
    }];
    let s = connect(&target, &opts(dir.path()), p.clone(), &MemStore::default())
        .await
        .unwrap();
    let out = s.exec("hello").await.unwrap();
    assert_eq!(out.stdout_str(), "hello");
    // Both hops verified their host key and logged in.
    assert_eq!(
        p.calls(),
        vec!["host_key", "password", "host_key", "password"]
    );
    let learned = std::fs::read_to_string(dir.path().join("known_hosts")).unwrap();
    assert_eq!(learned.lines().count(), 2, "{learned}");
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test ssh_session`
Expected: 编译失败（`MAX_KBD_ROUNDS`、`SshSession::notes` 等不存在）。

- [ ] **Step 3: 写 `crates/mai-core/src/ssh/signer.rs`**

```rust
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

use super::auth::{Prompter, SecretStore, passphrase_key};

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

/// Error type the russh `Signer` trait requires; never produced here.
#[derive(Debug)]
pub struct SignError;

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
        let AgentIdentity::PublicKey { key: public, .. } = key else {
            return Ok(to_sign);
        };
        Ok(bogus_signed(public, hash_alg, &to_sign))
    }
}
```

- [ ] **Step 4: `crates/mai-core/src/ssh/mod.rs`（完整内容）**

```rust
//! SSH access to monitored hosts: config resolution, host keys,
//! authentication and sessions.

pub mod auth;
pub mod client;
pub mod config;
pub mod hostkey;
mod pty;
pub mod signer;
```

- [ ] **Step 5: `crates/mai-core/src/ssh/auth.rs`（完整内容）**

```rust
//! User interaction and secret storage used while connecting.
//!
//! The app implements `Prompter` with dialogs; tests use scripted ones.
//! Secrets (passwords, key passphrases) live only in a `SecretStore`,
//! by default the OS keychain.

use std::fmt;
use std::future::Future;
use std::path::Path;

/// A secret typed by the user, and whether to remember it.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret {
    pub value: String,
    pub remember: bool,
}

impl fmt::Debug for Secret {
    /// Redacts `value`: secrets must never end up in logs or panic
    /// messages via a derived `Debug`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secret")
            .field("value", &"<redacted>")
            .field("remember", &self.remember)
            .finish()
    }
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

/// Store key for a private key's passphrase (`passphrase:<path>`). The
/// path is made canonical when the file exists, so different spellings of
/// the same file (relative, `..`, symlinks; on Windows also letter case
/// and `/` vs `\`) share one entry.
pub fn passphrase_key(key_file: &Path) -> String {
    let path = std::fs::canonicalize(key_file).unwrap_or_else(|_| key_file.to_path_buf());
    let mut text = path.display().to_string();
    if cfg!(windows) {
        // canonicalize returns a verbatim `\\?\C:\...` path.
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            text = rest.to_owned();
        }
        text = text.to_lowercase();
    }
    format!("passphrase:{text}")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_debug_redacts_value() {
        let s = Secret {
            value: "hunter2".into(),
            remember: true,
        };
        let debug = format!("{s:?}");
        assert!(!debug.contains("hunter2"), "{debug}");
        assert!(debug.contains("redacted"), "{debug}");
        assert!(debug.contains("true"), "{debug}");
    }
}
```

- [ ] **Step 6: `crates/mai-core/src/ssh/client.rs`（最终完整内容）**

```rust
//! SSH sessions: connect (optionally through jump hosts), verify the host
//! key, authenticate, and run commands.
//!
//! Auth order (design 3.2): private key files (passphrase from the secret
//! store or prompt), ssh-agent, remembered password, prompted password,
//! keyboard-interactive. Methods the server does not offer are skipped.

use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle, KeyboardInteractiveAuthResponse};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use russh::keys::{PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{ChannelMsg, MethodKind, MethodSet};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream};

use super::auth::{KbdPrompt, Prompter, SecretStore, password_key};
use super::config::HostSpec;
use super::hostkey::{self, HostKeyStatus};
use super::signer::{FileSigner, load_key, public_key_of};
use crate::pty::{PtyIo, TermSize};
use crate::stderr::StderrTail;

/// Where host keys are checked and learned, and how long to wait.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// Checked in order (e.g. `~/.ssh/known_hosts`, the app's own file).
    pub known_hosts: Vec<PathBuf>,
    /// Keys the user confirms are appended here.
    pub learn_to: PathBuf,
    /// TCP connect + key exchange timeout per hop.
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SshError {
    /// TCP, key exchange, or transport failure, including a transport
    /// error raised while an authentication call was in flight (the server
    /// or network dropped the connection, not a rejected credential).
    /// Retryable: the 03b host manager reconnects on this.
    Connect(String),
    /// The user declined an unknown host key.
    HostKeyRejected {
        host: String,
        port: u16,
        fingerprint: String,
    },
    /// known_hosts has a different key for this host: never connect.
    HostKeyChanged {
        host: String,
        port: u16,
        file: PathBuf,
        line: usize,
    },
    /// known_hosts marks this host key `@revoked`: never connect.
    HostKeyRevoked {
        host: String,
        port: u16,
        file: PathBuf,
        line: usize,
    },
    /// No authentication method succeeded: the server ran out of methods
    /// to offer. Not retryable by reconnecting; needs the user to supply
    /// different credentials. The 03b host manager treats this as
    /// `AuthRequired` and does not auto-retry.
    Auth(String),
    Channel(String),
}

impl fmt::Display for SshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(m) => write!(f, "connect: {m}"),
            Self::HostKeyRejected {
                host,
                port,
                fingerprint,
            } => {
                write!(f, "host key for {host}:{port} not trusted ({fingerprint})")
            }
            Self::HostKeyChanged {
                host,
                port,
                file,
                line,
            } => write!(
                f,
                "HOST KEY CHANGED for {host}:{port} (see {}:{line}); refusing to connect",
                file.display()
            ),
            Self::HostKeyRevoked {
                host,
                port,
                file,
                line,
            } => write!(
                f,
                "host key for {host}:{port} is REVOKED (see {}:{line}); refusing to connect",
                file.display()
            ),
            Self::Auth(m) => write!(f, "authentication: {m}"),
            Self::Channel(m) => write!(f, "channel: {m}"),
        }
    }
}

impl std::error::Error for SshError {}

fn chan_err(e: impl fmt::Display) -> SshError {
    SshError::Channel(e.to_string())
}

/// A `russh::Error` raised by an authentication call is a transport
/// failure, not a rejected credential: russh only reports rejection
/// through `AuthResult::Failure`. `SshError::Auth` is reserved for
/// "no method succeeded" once every offered method has been tried.
fn connect_err(e: impl fmt::Display) -> SshError {
    SshError::Connect(e.to_string())
}

/// Why a host key was refused, filled in by the handler.
type Verdict = Arc<Mutex<Option<SshError>>>;

const IDLE: u8 = 0;
const PROMPTING: u8 = 1;
const ABANDONED: u8 = 2;

/// Coordination between the connect timeout and the host-key prompt.
/// `phase` moves IDLE -> PROMPTING -> IDLE (handler) or IDLE -> ABANDONED
/// (timeout); both use compare-and-swap, so a timed-out connection never
/// prompts or learns a key, and a prompt is never cut short by the timer.
#[derive(Default)]
struct HopState {
    phase: AtomicU8,
    /// Incremented when a prompt finishes; the timer then starts afresh.
    prompts_done: AtomicU64,
}

/// Files to check a host key against: `known_hosts`, plus `learn_to` if it
/// isn't already one of them. This way a caller whose `learn_to` is not
/// listed in `known_hosts` is not re-prompted on every connect for a key
/// it already learned and wrote there itself.
fn check_files(opts: &ConnectOptions) -> Vec<PathBuf> {
    let mut files = opts.known_hosts.clone();
    if !files.contains(&opts.learn_to) {
        files.push(opts.learn_to.clone());
    }
    files
}

pub struct Checker<P> {
    host: String,
    port: u16,
    opts: ConnectOptions,
    prompter: Arc<P>,
    verdict: Verdict,
    state: Arc<HopState>,
}

impl<P: Prompter> client::Handler for Checker<P> {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            self.refuse(SshError::Connect(
                "host certificates are not supported".into(),
            ));
            return Ok(false);
        };
        match hostkey::check(&check_files(&self.opts), &self.host, self.port, key) {
            HostKeyStatus::Known => Ok(true),
            HostKeyStatus::Changed { file, line } => {
                self.refuse(SshError::HostKeyChanged {
                    host: self.host.clone(),
                    port: self.port,
                    file,
                    line,
                });
                Ok(false)
            }
            HostKeyStatus::Revoked { file, line } => {
                self.refuse(SshError::HostKeyRevoked {
                    host: self.host.clone(),
                    port: self.port,
                    file,
                    line,
                });
                Ok(false)
            }
            HostKeyStatus::Unreadable { file, error } => {
                // Fail closed: never let an unparsable file fall through
                // to an Unknown-host prompt. No prompt is shown.
                self.refuse(SshError::Connect(format!(
                    "cannot read known_hosts file {}: {error}",
                    file.display()
                )));
                Ok(false)
            }
            HostKeyStatus::Unknown => {
                let claimed = self.state.phase.compare_exchange(
                    IDLE,
                    PROMPTING,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                if claimed.is_err() {
                    // The connection already timed out: do not prompt.
                    return Ok(false);
                }
                let fp = hostkey::fingerprint(key);
                let trusted = self
                    .prompter
                    .confirm_host_key(&self.host, self.port, &fp)
                    .await;
                // Decide and learn while still PROMPTING, so the timer
                // cannot abandon the connection in between.
                let accepted = if !trusted {
                    self.refuse(SshError::HostKeyRejected {
                        host: self.host.clone(),
                        port: self.port,
                        fingerprint: fp,
                    });
                    false
                } else if let Err(e) =
                    hostkey::learn(&self.opts.learn_to, &self.host, self.port, key)
                {
                    self.refuse(SshError::Connect(format!("cannot record host key: {e}")));
                    false
                } else {
                    true
                };
                self.state.prompts_done.fetch_add(1, Ordering::SeqCst);
                self.state.phase.store(IDLE, Ordering::SeqCst);
                Ok(accepted)
            }
        }
    }
}

impl<P> Checker<P> {
    fn refuse(&self, e: SshError) {
        if let Ok(mut v) = self.verdict.lock() {
            *v = Some(e);
        }
    }
}

/// Output of a finished remote command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecOutput {
    pub status: Option<u32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl ExecOutput {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }
}

/// An authenticated SSH connection. Keeps its jump-host connections alive.
pub struct SshSession<P: Prompter> {
    handle: Handle<Checker<P>>,
    _via: Option<Box<SshSession<P>>>,
    notes: Vec<String>,
}

fn client_config() -> Arc<client::Config> {
    Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    })
}

/// Connect to `spec`, hopping through `spec.jumps` (outermost first).
/// Jump hosts' own `ProxyJump` settings are not followed.
pub async fn connect<P: Prompter, S: SecretStore>(
    spec: &HostSpec,
    opts: &ConnectOptions,
    prompter: Arc<P>,
    secrets: &S,
) -> Result<SshSession<P>, SshError> {
    let mut via: Option<SshSession<P>> = None;
    for hop in spec.jumps.iter().chain(std::iter::once(spec)) {
        let session = connect_hop(hop, opts, prompter.clone(), secrets, via.take()).await?;
        via = Some(session);
    }
    via.ok_or_else(|| SshError::Connect("no hops".into()))
}

async fn connect_hop<P: Prompter, S: SecretStore>(
    hop: &HostSpec,
    opts: &ConnectOptions,
    prompter: Arc<P>,
    secrets: &S,
    via: Option<SshSession<P>>,
) -> Result<SshSession<P>, SshError> {
    let verdict: Verdict = Arc::default();
    let state = Arc::new(HopState::default());
    let checker = Checker {
        host: hop.host.clone(),
        port: hop.port,
        opts: opts.clone(),
        prompter: prompter.clone(),
        verdict: verdict.clone(),
        state: state.clone(),
    };
    let result = {
        let connecting = async {
            match &via {
                None => {
                    client::connect(client_config(), (hop.host.as_str(), hop.port), checker).await
                }
                Some(outer) => {
                    let ch = outer
                        .handle
                        .channel_open_direct_tcpip(
                            hop.host.clone(),
                            u32::from(hop.port),
                            "127.0.0.1",
                            0,
                        )
                        .await?;
                    client::connect_stream(client_config(), ch.into_stream(), checker).await
                }
            }
        };
        // The timeout covers TCP connect and key exchange only: it waits
        // while the host-key prompt is open, and starts a full period again
        // after a prompt finishes. Giving up marks the hop ABANDONED so the
        // detached russh session task can no longer prompt or learn a key.
        tokio::pin!(connecting);
        let mut seen = state.prompts_done.load(Ordering::SeqCst);
        loop {
            tokio::select! {
                r = &mut connecting => break r,
                _ = tokio::time::sleep(opts.timeout) => {
                    let done = state.prompts_done.load(Ordering::SeqCst);
                    if done != seen {
                        seen = done;
                        continue;
                    }
                    let gave_up = state.phase.compare_exchange(
                        IDLE,
                        ABANDONED,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    );
                    if gave_up.is_ok() {
                        return Err(SshError::Connect(format!(
                            "{}:{} timed out",
                            hop.host, hop.port
                        )));
                    }
                }
            }
        }
    };
    let mut handle = match result {
        Err(e) => {
            let refused = verdict.lock().ok().and_then(|mut v| v.take());
            return Err(refused
                .unwrap_or_else(|| SshError::Connect(format!("{}:{}: {e}", hop.host, hop.port))));
        }
        Ok(h) => h,
    };
    let mut notes = via.as_ref().map(|v| v.notes.clone()).unwrap_or_default();
    authenticate(&mut handle, hop, prompter.as_ref(), secrets, &mut notes).await?;
    Ok(SshSession {
        handle,
        _via: via.map(Box::new),
        notes,
    })
}

fn offers(methods: &Option<MethodSet>, kind: MethodKind) -> bool {
    methods.as_ref().is_none_or(|m| m.contains(&kind))
}

/// Result of one authentication attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Fully authenticated.
    Done,
    /// This factor was accepted but the server wants more (e.g. 2FA).
    Partial,
    Failed,
}

/// Record the server's remaining methods and classify the result.
fn step(methods: &mut Option<MethodSet>, r: &client::AuthResult) -> Step {
    match r {
        client::AuthResult::Success => Step::Done,
        client::AuthResult::Failure {
            remaining_methods,
            partial_success,
        } => {
            *methods = Some(remaining_methods.clone());
            if *partial_success {
                Step::Partial
            } else {
                Step::Failed
            }
        }
    }
}

async fn authenticate<P: Prompter, S: SecretStore>(
    h: &mut Handle<Checker<P>>,
    spec: &HostSpec,
    prompter: &P,
    secrets: &S,
    notes: &mut Vec<String>,
) -> Result<(), SshError> {
    let user = spec.user.as_str();
    let mut methods = match h.authenticate_none(user).await.map_err(connect_err)? {
        client::AuthResult::Success => return Ok(()),
        client::AuthResult::Failure {
            remaining_methods, ..
        } => Some(remaining_methods),
    };
    'keys: {
        if !offers(&methods, MethodKind::PublicKey) {
            break 'keys;
        }
        // Public keys already offered, so the agent does not offer them
        // again (each attempt counts against MaxAuthTries).
        let mut tried: Vec<PublicKey> = Vec::new();
        for path in spec.identity_files.iter().filter(|p| p.is_file()) {
            let hash = h
                .best_supported_rsa_hash()
                .await
                .map_err(connect_err)?
                .flatten();
            let r = match public_key_of(path) {
                // Offer the public key; the passphrase is asked only if the
                // server accepts it.
                Some(public) => {
                    tried.push(public.clone());
                    let mut signer = FileSigner {
                        path,
                        prompter,
                        secrets,
                        notes,
                    };
                    match h
                        .authenticate_publickey_with(user, public, hash, &mut signer)
                        .await
                    {
                        Ok(r) => r,
                        Err(_) => return Err(SshError::Connect("connection lost".into())),
                    }
                }
                // Formats without a readable public key: decrypt first.
                None => {
                    let Some(key) = load_key(path, prompter, secrets, notes).await else {
                        continue;
                    };
                    tried.push(key.public_key().clone());
                    h.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
                        .await
                        .map_err(connect_err)?
                }
            };
            match step(&mut methods, &r) {
                Step::Done => return Ok(()),
                Step::Partial => break 'keys,
                Step::Failed => {}
            }
        }
        match try_agent(h, user, &tried, &mut methods).await? {
            Step::Done => return Ok(()),
            Step::Partial | Step::Failed => {}
        }
    }

    if offers(&methods, MethodKind::Password) {
        let key = password_key(&spec.alias);
        let mut accepted = false;
        if let Some(pw) = secrets.get(&key) {
            let r = h
                .authenticate_password(user, pw)
                .await
                .map_err(connect_err)?;
            match step(&mut methods, &r) {
                Step::Done => return Ok(()),
                Step::Partial => accepted = true,
                Step::Failed => {
                    if let Err(e) = secrets.delete(&key) {
                        notes.push(format!(
                            "could not remove the wrong password of {} from the keychain: {e}",
                            spec.alias
                        ));
                    }
                }
            }
        }
        if !accepted
            && offers(&methods, MethodKind::Password)
            && let Some(s) = prompter.password(user, &spec.host).await
        {
            let r = h
                .authenticate_password(user, s.value.clone())
                .await
                .map_err(connect_err)?;
            let result = step(&mut methods, &r);
            if result != Step::Failed
                && s.remember
                && let Err(e) = secrets.set(&key, &s.value)
            {
                notes.push(format!(
                    "could not save the password of {} in the keychain: {e}",
                    spec.alias
                ));
            }
            if result == Step::Done {
                return Ok(());
            }
        }
    }

    if offers(&methods, MethodKind::KeyboardInteractive)
        && keyboard_interactive(h, spec, prompter).await?
    {
        return Ok(());
    }
    Err(SshError::Auth(format!(
        "no method succeeded for {user}@{}",
        spec.host
    )))
}

/// Offer the agent's keys that were not tried already. Partial success
/// (the server wants another factor) stops here like a key file does.
async fn agent_auth<P: Prompter, A>(
    h: &mut Handle<Checker<P>>,
    user: &str,
    mut agent: AgentClient<A>,
    tried: &[PublicKey],
    methods: &mut Option<MethodSet>,
) -> Result<Step, SshError>
where
    A: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Ok(ids) = agent.request_identities().await else {
        return Ok(Step::Failed);
    };
    for id in ids {
        let AgentIdentity::PublicKey { key, .. } = id else {
            continue;
        };
        if tried.iter().any(|t| t.key_data() == key.key_data()) {
            continue;
        }
        let hash = h
            .best_supported_rsa_hash()
            .await
            .map_err(connect_err)?
            .flatten();
        if let Ok(r) = h
            .authenticate_publickey_with(user, key, hash, &mut agent)
            .await
        {
            match step(methods, &r) {
                Step::Failed => {}
                done_or_partial => return Ok(done_or_partial),
            }
        }
    }
    Ok(Step::Failed)
}

#[cfg(unix)]
async fn try_agent<P: Prompter>(
    h: &mut Handle<Checker<P>>,
    user: &str,
    tried: &[PublicKey],
    methods: &mut Option<MethodSet>,
) -> Result<Step, SshError> {
    match AgentClient::connect_env().await {
        Ok(agent) => agent_auth(h, user, agent, tried, methods).await,
        Err(_) => Ok(Step::Failed),
    }
}

#[cfg(windows)]
async fn try_agent<P: Prompter>(
    h: &mut Handle<Checker<P>>,
    user: &str,
    tried: &[PublicKey],
    methods: &mut Option<MethodSet>,
) -> Result<Step, SshError> {
    if let Ok(agent) = AgentClient::connect_named_pipe(r"\\.\pipe\openssh-ssh-agent").await {
        let r = agent_auth(h, user, agent, tried, methods).await?;
        if r != Step::Failed {
            return Ok(r);
        }
    }
    match AgentClient::connect_pageant().await {
        Ok(agent) => agent_auth(h, user, agent, tried, methods).await,
        Err(_) => Ok(Step::Failed),
    }
}

/// A server that keeps asking keyboard-interactive questions is given up
/// on after this many rounds.
pub const MAX_KBD_ROUNDS: usize = 10;

async fn keyboard_interactive<P: Prompter>(
    h: &mut Handle<Checker<P>>,
    spec: &HostSpec,
    prompter: &P,
) -> Result<bool, SshError> {
    let mut reply = h
        .authenticate_keyboard_interactive_start(spec.user.clone(), None)
        .await
        .map_err(connect_err)?;
    for _ in 0..MAX_KBD_ROUNDS {
        match reply {
            KeyboardInteractiveAuthResponse::Success => return Ok(true),
            KeyboardInteractiveAuthResponse::Failure { .. } => return Ok(false),
            KeyboardInteractiveAuthResponse::InfoRequest {
                name,
                instructions,
                prompts,
            } => {
                let asks: Vec<KbdPrompt> = prompts
                    .iter()
                    .map(|p| KbdPrompt {
                        text: p.prompt.clone(),
                        echo: p.echo,
                    })
                    .collect();
                let answers = if asks.is_empty() {
                    Vec::new()
                } else {
                    match prompter
                        .keyboard_interactive(&name, &instructions, &asks)
                        .await
                    {
                        Some(a) => a,
                        None => return Ok(false),
                    }
                };
                reply = h
                    .authenticate_keyboard_interactive_respond(answers)
                    .await
                    .map_err(connect_err)?;
            }
        }
    }
    Ok(matches!(reply, KeyboardInteractiveAuthResponse::Success))
}

impl<P: Prompter> SshSession<P> {
    /// Things the user should know that did not stop the connection, from
    /// every hop (e.g. a password that could not be saved in the keychain).
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Run `command` to completion, collecting stdout, stderr and status.
    pub async fn exec(&self, command: &str) -> Result<ExecOutput, SshError> {
        let mut ch = self.handle.channel_open_session().await.map_err(chan_err)?;
        ch.exec(true, command).await.map_err(chan_err)?;
        let mut out = ExecOutput::default();
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.stdout.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, .. } => out.stderr.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => out.status = Some(exit_status),
                _ => {}
            }
        }
        Ok(out)
    }

    /// Start `command` and hand back its channel for streaming (stdin via
    /// `data`, stdout via `wait`). `env` is requested first; servers that
    /// do not accept a variable ignore it.
    pub async fn open_exec(
        &self,
        command: &str,
        env: &[(&str, &str)],
    ) -> Result<russh::Channel<client::Msg>, SshError> {
        let ch = self.handle.channel_open_session().await.map_err(chan_err)?;
        for (k, v) in env {
            ch.set_env(false, *k, *v).await.map_err(chan_err)?;
        }
        ch.exec(true, command).await.map_err(chan_err)?;
        Ok(ch)
    }

    /// Like `open_exec`, but split into a stdout byte stream and a stdin
    /// writer, with stderr collected into `stderr`.
    pub async fn open_exec_split(
        &self,
        command: &str,
        env: &[(&str, &str)],
        stderr: Arc<StderrTail>,
    ) -> Result<(DuplexStream, Box<dyn AsyncWrite + Send + Unpin>), SshError> {
        let (mut read, write) = self.open_exec(command, env).await?.split();
        let writer: Box<dyn AsyncWrite + Send + Unpin> = Box::new(write.make_writer());
        let (mut stdout, reader) = tokio::io::duplex(64 * 1024);
        tokio::spawn(async move {
            // The write half lives as long as the channel is read.
            let _write = write;
            while let Some(msg) = read.wait().await {
                match msg {
                    ChannelMsg::Data { data } => {
                        if stdout.write_all(&data).await.is_err() {
                            break;
                        }
                    }
                    ChannelMsg::ExtendedData { data, .. } => stderr.push(&data),
                    ChannelMsg::Eof | ChannelMsg::Close => break,
                    _ => {}
                }
            }
            stderr.finish();
        });
        Ok((reader, writer))
    }

    /// Run a command in a PTY of `size` (terminal type `xterm-256color`).
    /// The variables in `env` are requested first and each answer is
    /// awaited; `command` receives whether all were accepted, so it can
    /// fall back to setting them in the command line.
    pub async fn open_pty(
        &self,
        size: TermSize,
        env: &[(&str, &str)],
        command: impl FnOnce(bool) -> String,
    ) -> Result<PtyIo, SshError> {
        let ch = self.handle.channel_open_session().await.map_err(chan_err)?;
        super::pty::start(ch, size, env, command).await
    }

    /// Open an SFTP session. Relative paths are relative to the login
    /// directory (the home directory on OpenSSH servers).
    pub async fn sftp(&self) -> Result<russh_sftp::client::SftpSession, SshError> {
        let ch = self.handle.channel_open_session().await.map_err(chan_err)?;
        ch.request_subsystem(true, "sftp").await.map_err(chan_err)?;
        russh_sftp::client::SftpSession::new(ch.into_stream())
            .await
            .map_err(chan_err)
    }

    pub async fn close(self) {
        let _ = self
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "en")
            .await;
    }
}

// `step` is tested here because the in-process russh test server never
// sends `partial_success = true` (russh 0.63 resets it), so the 2FA path
// cannot be driven end-to-end in tests/ssh_session.rs.
#[cfg(test)]
mod tests {
    use super::*;

    fn failure(methods: &[MethodKind], partial: bool) -> client::AuthResult {
        client::AuthResult::Failure {
            remaining_methods: MethodSet::from(methods),
            partial_success: partial,
        }
    }

    #[test]
    fn step_classifies_success_partial_and_failure() {
        let mut methods = None;
        assert_eq!(step(&mut methods, &client::AuthResult::Success), Step::Done);
        assert_eq!(methods, None);

        let r = failure(&[MethodKind::KeyboardInteractive], true);
        assert_eq!(step(&mut methods, &r), Step::Partial);
        assert!(!offers(&methods, MethodKind::Password));
        assert!(offers(&methods, MethodKind::KeyboardInteractive));

        let r = failure(&[MethodKind::Password], false);
        assert_eq!(step(&mut methods, &r), Step::Failed);
        assert!(offers(&methods, MethodKind::Password));
    }

    #[test]
    fn unknown_method_set_offers_everything() {
        assert!(offers(&None, MethodKind::PublicKey));
        assert!(offers(&None, MethodKind::Password));
    }
}
```

- [ ] **Step 7: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: `ssh_session` 19 个测试通过，全部 232 个；clippy 无警告。

- [ ] **Step 8: 提交**

```bash
git add crates/mai-core
git commit -m "feat(core): probe public keys before decrypting; agent dedupe, partial success, kbd limit

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: 部署加固（B11、B12、B18、B19）

**Files:**

- Create: `crates/mai-core/src/swap.rs`、`crates/mai-core/tests/swap.rs`
- Modify: `crates/mai-core/src/deploy.rs`、`crates/mai-core/src/connect.rs`、
  `crates/mai-core/src/lib.rs`
- Test: `crates/mai-core/tests/deploy.rs`、`crates/mai-core/tests/connect.rs`

**Interfaces:**

- Consumes: Task 1 的 `connect.rs`。
- Produces:

  ```rust
  // swap
  pub trait BinFiles: Sync {             // 路径以 / 分隔
      fn exists(&self, path: &str) -> impl Future<Output = io::Result<bool>> + Send;
      fn remove(&self, path: &str) -> impl Future<Output = io::Result<()>> + Send;
      fn rename(&self, from: &str, to: &str) -> impl Future<Output = io::Result<()>> + Send;
      fn list(&self, dir: &str) -> impl Future<Output = io::Result<Vec<String>>> + Send;
  }
  pub async fn swap_in<F: BinFiles>(files: &F, dir: &str, exe: &str, stamp: u64) -> io::Result<()>
  pub struct LocalFiles;
  pub struct SftpFiles<'a>(pub &'a russh_sftp::client::SftpSession);
  // deploy
  pub const BEGIN_MARK: &str = "MAI-DETECT-BEGIN";
  pub const END_MARK: &str = "MAI-DETECT-END";
  pub fn posix_detect_command() -> String
  pub fn windows_detect_command(shell: Shell) -> String
  pub fn marked_lines(out: &str) -> Option<Vec<String>>
  pub fn parse_posix_detect(out: &str) -> Result<Option<(Os, String, String)>, DeployError>
  pub fn parse_windows_detect(out: &str) -> Result<(String, String), DeployError>
  // connect
  pub async fn place_binary(exe: &Path, bytes: &[u8], stop: impl Future<Output = ()>)
      -> std::io::Result<bool>
  ```

  `parse_posix_detect`：没有哨兵为 `Ok(None)`（不是 POSIX shell）；有哨兵但
  系统不受支持或 home 为空为 `Err(Detect)`。

- [ ] **Step 1: 写失败测试**

`crates/mai-core/tests/swap.rs`：

```rust
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
```

`crates/mai-core/tests/deploy.rs`（完整内容）：

```rust
use std::path::PathBuf;

use mai_core::deploy::{
    DeployError, HookResult, Os, ProbeStore, Remote, Shell, hooks_result, marked_lines,
    normalize_arch, parse_hash, parse_hook_lines, parse_posix_detect, parse_uname,
    parse_windows_detect, posix_detect_command, sha256_hex, stop_args, windows_detect_command,
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

#[test]
fn serve_and_stop_arguments_are_quoted_per_shell() {
    let posix = remote(Os::Linux, Shell::Posix, "/home/u");
    assert_eq!(
        posix.serve_args("mac-1", Some("/opt/it's/zellij")),
        r"serve --client mac-1 --zellij '/opt/it'\''s/zellij'"
    );
    let cmd = remote(Os::Windows, Shell::Cmd, r"C:\Users\u");
    assert_eq!(
        cmd.serve_args("a b", Some(r"C:\Program Files\zellij.exe")),
        r#"serve --client ab --zellij "C:\Program Files\zellij.exe""#
    );
    let ps = remote(Os::Windows, Shell::PowerShell, r"C:\Users\u");
    assert_eq!(
        ps.serve_args("", Some(r"C:\o'k\z.exe")),
        r"serve --zellij 'C:\o''k\z.exe'"
    );
    assert_eq!(posix.serve_args("", None), "serve");
    assert_eq!(stop_args("mac-1"), "stop --client mac-1");
    assert_eq!(stop_args("../"), "stop");
}

#[test]
fn posix_detection_ignores_shell_noise_around_the_markers() {
    let out = "Welcome to Ubuntu!\nLast login: today\nMAI-DETECT-BEGIN\nLinux x86_64\n/home/u\nMAI-DETECT-END\nbye\n";
    assert_eq!(
        parse_posix_detect(out).unwrap(),
        Some((Os::Linux, "x86_64".into(), "/home/u".into()))
    );
}

#[test]
fn posix_detection_without_markers_is_not_posix() {
    // cmd echoes the whole command line back on one line.
    let cmd_echo = format!("{}\r\n", posix_detect_command());
    assert_eq!(parse_posix_detect(&cmd_echo).unwrap(), None);
    assert_eq!(parse_posix_detect("").unwrap(), None);
}

#[test]
fn posix_detection_errors_name_the_problem() {
    let unknown = "MAI-DETECT-BEGIN\nFreeBSD amd64\n/home/u\nMAI-DETECT-END\n";
    match parse_posix_detect(unknown) {
        Err(DeployError::Detect(m)) => assert!(m.contains("FreeBSD"), "{m}"),
        other => panic!("{other:?}"),
    }
    let no_home = "MAI-DETECT-BEGIN\nDarwin arm64\n\nMAI-DETECT-END\n";
    assert!(parse_posix_detect(no_home).is_err());
}

#[test]
fn windows_detection_and_unset_userprofile() {
    let ok = "MAI-DETECT-BEGIN\r\nAMD64\r\nC:\\Users\\u\r\nMAI-DETECT-END\r\n";
    assert_eq!(
        parse_windows_detect(ok).unwrap(),
        ("x86_64".into(), r"C:\Users\u".into())
    );
    // cmd echoes an unset variable back verbatim.
    let unset = "MAI-DETECT-BEGIN\r\nAMD64\r\n%USERPROFILE%\r\nMAI-DETECT-END\r\n";
    match parse_windows_detect(unset) {
        Err(DeployError::Detect(m)) => assert!(m.contains("USERPROFILE"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn detection_commands_and_markers() {
    assert!(posix_detect_command().contains("uname -sm"));
    assert!(windows_detect_command(Shell::Cmd).contains("%USERPROFILE%"));
    assert!(windows_detect_command(Shell::PowerShell).contains("$env:USERPROFILE"));
    assert_eq!(
        marked_lines("x\nMAI-DETECT-BEGIN\na\n b \nMAI-DETECT-END\ny"),
        Some(vec!["a".to_owned(), "b".to_owned()])
    );
    assert_eq!(marked_lines("MAI-DETECT-BEGIN\na\n"), None, "no end marker");
}
```

`crates/mai-core/tests/connect.rs`（最终完整内容，`place_binary` 测试改为 async）：

```rust
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

#[tokio::test]
async fn binary_is_placed_only_when_it_differs() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join(".mai").join("bin").join("mai-probe");
    let stops = Cell::new(0);
    let stop = || async { stops.set(stops.get() + 1) };

    assert!(place_binary(&exe, b"v1", stop()).await.unwrap());
    assert_eq!(std::fs::read(&exe).unwrap(), b"v1");
    assert_eq!(stops.get(), 0, "nothing to stop on first install");

    assert!(!place_binary(&exe, b"v1", stop()).await.unwrap());
    assert_eq!(stops.get(), 0);

    assert!(place_binary(&exe, b"v2", stop()).await.unwrap());
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

#[test]
fn revoked_host_key_needs_the_user() {
    assert_eq!(
        ssh_open_error(SshError::HostKeyRevoked {
            host: "h".into(),
            port: 22,
            file: PathBuf::from("kh"),
            line: 4,
        }),
        OpenError::NeedsUser(Problem::HostKeyRevoked {
            file: PathBuf::from("kh"),
            line: 4
        })
    );
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test swap --test deploy --test connect`
Expected: 编译失败（`swap` 模块、`parse_posix_detect` 等不存在）。

- [ ] **Step 3: 写 `crates/mai-core/src/swap.rs`**

```rust
//! Put a freshly written probe binary in place without deleting the old
//! one first (B19). On Windows a running executable cannot be deleted but
//! can be renamed, so the old binary is moved aside to `<exe>.old` (or
//! `<exe>.old-<stamp>` when an older `.old` is still locked), the new one
//! is renamed into place, and the old one is removed when possible.
//! Leftover `<exe>.old*` files are removed on the next swap.

use std::future::Future;
use std::io;

/// The file operations a swap needs, on `/`-separated paths.
pub trait BinFiles: Sync {
    fn exists(&self, path: &str) -> impl Future<Output = io::Result<bool>> + Send;
    fn remove(&self, path: &str) -> impl Future<Output = io::Result<()>> + Send;
    fn rename(&self, from: &str, to: &str) -> impl Future<Output = io::Result<()>> + Send;
    /// File names in `dir`.
    fn list(&self, dir: &str) -> impl Future<Output = io::Result<Vec<String>>> + Send;
}

/// Replace `<dir>/<exe>` with `<dir>/<exe>.upload`. `stamp` makes the
/// aside name unique when needed (e.g. milliseconds since the epoch).
pub async fn swap_in<F: BinFiles>(files: &F, dir: &str, exe: &str, stamp: u64) -> io::Result<()> {
    let target = format!("{dir}/{exe}");
    let tmp = format!("{target}.upload");
    let old_prefix = format!("{exe}.old");
    // Best effort: leftovers of earlier swaps (still locked ones stay).
    if let Ok(names) = files.list(dir).await {
        for name in names.iter().filter(|n| n.starts_with(&old_prefix)) {
            let _ = files.remove(&format!("{dir}/{name}")).await;
        }
    }
    let mut aside = None;
    if files.exists(&target).await? {
        let mut name = format!("{target}.old");
        if files.exists(&name).await? {
            name = format!("{target}.old-{stamp}");
        }
        files.rename(&target, &name).await?;
        aside = Some(name);
    }
    if let Err(e) = files.rename(&tmp, &target).await {
        // Put the old binary back so the host keeps a working probe.
        if let Some(name) = &aside {
            let _ = files.rename(name, &target).await;
        }
        return Err(e);
    }
    if let Some(name) = aside {
        let _ = files.remove(&name).await;
    }
    Ok(())
}

/// `BinFiles` on the local file system.
pub struct LocalFiles;

impl BinFiles for LocalFiles {
    async fn exists(&self, path: &str) -> io::Result<bool> {
        tokio::fs::try_exists(path).await
    }

    async fn remove(&self, path: &str) -> io::Result<()> {
        tokio::fs::remove_file(path).await
    }

    async fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        tokio::fs::rename(from, to).await
    }

    async fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(e) = entries.next_entry().await? {
            names.push(e.file_name().to_string_lossy().into_owned());
        }
        Ok(names)
    }
}

/// `BinFiles` over SFTP (paths relative to the login directory).
pub struct SftpFiles<'a>(pub &'a russh_sftp::client::SftpSession);

fn sftp_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

impl BinFiles for SftpFiles<'_> {
    async fn exists(&self, path: &str) -> io::Result<bool> {
        self.0.try_exists(path.to_owned()).await.map_err(sftp_err)
    }

    async fn remove(&self, path: &str) -> io::Result<()> {
        self.0.remove_file(path.to_owned()).await.map_err(sftp_err)
    }

    async fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        self.0
            .rename(from.to_owned(), to.to_owned())
            .await
            .map_err(sftp_err)
    }

    async fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        let entries = self.0.read_dir(dir.to_owned()).await.map_err(sftp_err)?;
        Ok(entries.map(|e| e.file_name()).collect())
    }
}
```

- [ ] **Step 4: `crates/mai-core/src/deploy.rs`（完整内容）**

```rust
//! Install mai-probe on a remote host (design 4.2): detect OS, CPU and
//! login shell, compare SHA-256 with the bundled binary, upload over SFTP
//! when different, then run `install-hooks`. When an existing probe
//! differs, `mai-probe stop --client <id>` is run first (best effort) so
//! a running probe does not lock the file.

use std::fmt;
use std::path::PathBuf;

use mai_protocol::sanitize_client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::ssh::auth::Prompter;
use crate::ssh::client::{ExecOutput, SshError, SshSession};
use crate::swap::{SftpFiles, swap_in};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

/// How commands sent over `exec` are interpreted on the remote side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Posix,
    Cmd,
    PowerShell,
}

/// What `detect` learned about the remote host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    pub os: Os,
    /// `x86_64` or `aarch64`.
    pub arch: String,
    pub shell: Shell,
    /// Absolute home directory in the remote OS's own syntax.
    pub home: String,
}

impl Remote {
    /// Rust target triple of the probe binary this host needs.
    pub fn target(&self) -> Option<&'static str> {
        Some(match (self.os, self.arch.as_str()) {
            (Os::Linux, "x86_64") => "x86_64-unknown-linux-musl",
            (Os::Linux, "aarch64") => "aarch64-unknown-linux-musl",
            (Os::MacOs, "x86_64") => "x86_64-apple-darwin",
            (Os::MacOs, "aarch64") => "aarch64-apple-darwin",
            (Os::Windows, "x86_64") => "x86_64-pc-windows-msvc",
            _ => return None,
        })
    }

    fn exe_name(&self) -> &'static str {
        if self.os == Os::Windows {
            "mai-probe.exe"
        } else {
            "mai-probe"
        }
    }

    /// Absolute path of the probe under `<home>/<dir>/bin/`.
    pub fn probe_path(&self, dir: &str) -> String {
        if self.os == Os::Windows {
            format!(
                "{}\\{}\\bin\\{}",
                self.home,
                dir.replace('/', "\\"),
                self.exe_name()
            )
        } else {
            format!("{}/{dir}/bin/{}", self.home, self.exe_name())
        }
    }

    /// Command line that runs the program at `path` with `args`.
    pub fn invoke(&self, path: &str, args: &str) -> String {
        match self.shell {
            Shell::Posix => format!("{} {args}", sh_quote(path)),
            Shell::Cmd => format!("\"{path}\" {args}"),
            Shell::PowerShell => format!("& '{}' {args}", path.replace('\'', "''")),
        }
    }

    /// Quote one argument for this host's shell. Under cmd the value is
    /// wrapped in double quotes without escaping (Windows paths cannot
    /// contain `"`).
    pub fn quote_arg(&self, arg: &str) -> String {
        match self.shell {
            Shell::Posix => sh_quote(arg),
            Shell::Cmd => format!("\"{arg}\""),
            Shell::PowerShell => format!("'{}'", arg.replace('\'', "''")),
        }
    }

    /// Join `argv` for this host's shell, quoting every word that is not
    /// plain `[A-Za-z0-9_-]`.
    pub fn join_args(&self, argv: &[String]) -> String {
        let plain = |w: &str| {
            !w.is_empty()
                && w.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        };
        argv.iter()
            .map(|w| {
                if plain(w) {
                    w.clone()
                } else {
                    self.quote_arg(w)
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `serve_argv` joined for this host's shell.
    pub fn serve_args(&self, client: &str, zellij: Option<&str>) -> String {
        self.join_args(&serve_argv(client, zellij))
    }

    /// Command printing the SHA-256 of `path` (parse with `parse_hash`).
    pub fn hash_command(&self, path: &str) -> String {
        match (self.os, self.shell) {
            (Os::MacOs, _) => format!("shasum -a 256 {}", sh_quote(path)),
            (_, Shell::Posix) => format!("sha256sum {}", sh_quote(path)),
            (_, Shell::Cmd) => format!("certutil -hashfile \"{path}\" SHA256"),
            (_, Shell::PowerShell) => {
                format!(
                    "(Get-FileHash -Algorithm SHA256 '{}').Hash",
                    path.replace('\'', "''")
                )
            }
        }
    }
}

/// Single-quote for POSIX sh.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Parse `uname -sm` (e.g. `Darwin arm64`, `Linux x86_64`).
pub fn parse_uname(out: &str) -> Option<(Os, String)> {
    let mut it = out.split_whitespace();
    let os = match it.next()? {
        "Linux" => Os::Linux,
        "Darwin" => Os::MacOs,
        _ => return None,
    };
    Some((os, normalize_arch(it.next()?)?))
}

/// Map `uname -m` / `PROCESSOR_ARCHITECTURE` values to `x86_64`/`aarch64`.
pub fn normalize_arch(raw: &str) -> Option<String> {
    Some(
        match raw.trim().to_ascii_lowercase().as_str() {
            "x86_64" | "amd64" => "x86_64",
            "aarch64" | "arm64" => "aarch64",
            _ => return None,
        }
        .to_owned(),
    )
}

/// First 64-hex-digit token (sha256sum, shasum, certutil, Get-FileHash),
/// lowercased.
pub fn parse_hash(out: &str) -> Option<String> {
    out.split_whitespace()
        .find(|t| t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()))
        .map(str::to_ascii_lowercase)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Bundled probe binaries: `<dir>/<target>/mai-probe[.exe]`.
#[derive(Debug, Clone)]
pub struct ProbeStore {
    pub dir: PathBuf,
}

impl ProbeStore {
    pub fn binary(&self, target: &str) -> PathBuf {
        let name = if target.contains("windows") {
            "mai-probe.exe"
        } else {
            "mai-probe"
        };
        self.dir.join(target).join(name)
    }
}

/// One line printed by `mai-probe install-hooks`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct HookResult {
    pub agent: String,
    /// installed | removed | unchanged | skipped | error
    pub outcome: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
}

/// Parse `install-hooks` output; non-JSON lines are ignored.
pub fn parse_hook_lines(out: &str) -> Vec<HookResult> {
    out.lines()
        .filter_map(|l| serde_json::from_str(l.trim()).ok())
        .collect()
}

/// Turn the finished `install-hooks` exec into its parsed results, or a
/// `DeployError::Hooks` if the command itself failed (a non-zero or
/// missing exit status is never silently treated as "no hooks installed").
pub fn hooks_result(out: &ExecOutput) -> Result<Vec<HookResult>, DeployError> {
    if !out.success() {
        return Err(DeployError::Hooks {
            status: out.status,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    Ok(parse_hook_lines(&out.stdout_str()))
}

#[derive(Debug, Clone)]
pub struct DeployOptions {
    /// Directory under the remote home; `.mai` in production (hooks are
    /// only recognised under `.mai/bin`).
    pub dir: String,
    pub install_hooks: bool,
    /// App installation id; its running `serve` is stopped before the
    /// binary is replaced (a running probe locks its file on Windows).
    pub client: String,
}

impl Default for DeployOptions {
    fn default() -> Self {
        Self {
            dir: ".mai".into(),
            install_hooks: true,
            client: String::new(),
        }
    }
}

/// Arguments of `mai-probe stop` for `client` (see `Remote::serve_args`).
pub fn stop_args(client: &str) -> String {
    stop_argv(client).join(" ")
}

/// `--client <id>` with the id reduced to `[A-Za-z0-9_-]`; nothing for
/// an empty id (PowerShell 5.1 drops empty arguments to native programs).
fn client_argv(client: &str) -> Vec<String> {
    let client = sanitize_client(client);
    if client.is_empty() {
        Vec::new()
    } else {
        vec!["--client".to_owned(), client]
    }
}

/// Arguments of `mai-probe serve` for `client`, with an optional
/// zellij path.
pub fn serve_argv(client: &str, zellij: Option<&str>) -> Vec<String> {
    let mut argv = vec!["serve".to_owned()];
    argv.extend(client_argv(client));
    if let Some(z) = zellij {
        argv.push("--zellij".to_owned());
        argv.push(z.to_owned());
    }
    argv
}

/// Arguments of `mai-probe stop` for `client`.
pub fn stop_argv(client: &str) -> Vec<String> {
    let mut argv = vec!["stop".to_owned()];
    argv.extend(client_argv(client));
    argv
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployReport {
    pub remote: Remote,
    pub probe_path: String,
    /// False when the remote binary already matched.
    pub uploaded: bool,
    pub hooks: Vec<HookResult>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeployError {
    Detect(String),
    NoBinary {
        target: String,
        path: PathBuf,
    },
    Upload(String),
    /// `install-hooks` exited with a non-zero (or missing) status.
    Hooks {
        status: Option<u32>,
        stderr: String,
    },
    Ssh(SshError),
}

impl fmt::Display for DeployError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Detect(m) => write!(f, "detect remote platform: {m}"),
            Self::NoBinary { target, path } => {
                write!(f, "no probe for {target} (expected {})", path.display())
            }
            Self::Upload(m) => write!(f, "upload probe: {m}"),
            Self::Hooks { status, stderr } => {
                write!(f, "install-hooks failed (status {status:?}): {stderr}")
            }
            Self::Ssh(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DeployError {}

impl From<SshError> for DeployError {
    fn from(e: SshError) -> Self {
        Self::Ssh(e)
    }
}

/// Start and end markers around detection output, so text a shell's
/// startup files print is not mistaken for it (B11).
pub const BEGIN_MARK: &str = "MAI-DETECT-BEGIN";
pub const END_MARK: &str = "MAI-DETECT-END";

/// One POSIX command printing `uname -sm` and `$HOME` between markers.
/// Under cmd the whole line is echoed back on one line, and under
/// PowerShell `uname` fails, so neither yields the marked lines.
pub fn posix_detect_command() -> String {
    format!("echo {BEGIN_MARK}; uname -sm; printf '%s\\n' \"$HOME\"; echo {END_MARK}")
}

/// Detection commands per Windows shell: CPU and home between markers.
pub fn windows_detect_command(shell: Shell) -> String {
    match shell {
        Shell::Cmd => format!(
            "echo {BEGIN_MARK}& echo %PROCESSOR_ARCHITECTURE%& echo %USERPROFILE%& echo {END_MARK}"
        ),
        _ => format!(
            "echo {BEGIN_MARK}; $env:PROCESSOR_ARCHITECTURE; $env:USERPROFILE; echo {END_MARK}"
        ),
    }
}

/// The trimmed lines between the first `BEGIN_MARK` line and the next
/// `END_MARK` line, or `None` if the markers are missing.
pub fn marked_lines(out: &str) -> Option<Vec<String>> {
    let mut lines = out.lines().map(str::trim);
    lines.find(|l| *l == BEGIN_MARK)?;
    let mut inner = Vec::new();
    for l in lines {
        if l == END_MARK {
            return Some(inner);
        }
        inner.push(l.to_owned());
    }
    None
}

/// OS, CPU and home from `posix_detect_command` output. `Ok(None)` when
/// the output is not from a POSIX shell (no markers).
pub fn parse_posix_detect(out: &str) -> Result<Option<(Os, String, String)>, DeployError> {
    let Some(lines) = marked_lines(out) else {
        return Ok(None);
    };
    let [uname, home] = lines.as_slice() else {
        return Err(DeployError::Detect(format!(
            "unexpected detection output: {lines:?}"
        )));
    };
    let (os, arch) = parse_uname(uname)
        .ok_or_else(|| DeployError::Detect(format!("unsupported system (uname: {uname:?})")))?;
    if home.is_empty() {
        return Err(DeployError::Detect("empty $HOME".into()));
    }
    Ok(Some((os, arch, home.clone())))
}

/// CPU and home from `windows_detect_command` output. Under cmd an unset
/// variable is echoed back as `%NAME%`, which counts as missing (B12).
pub fn parse_windows_detect(out: &str) -> Result<(String, String), DeployError> {
    let lines = marked_lines(out)
        .ok_or_else(|| DeployError::Detect(format!("unexpected detection output: {out:?}")))?;
    let [raw_arch, home] = lines.as_slice() else {
        return Err(DeployError::Detect(format!(
            "unexpected detection output: {lines:?}"
        )));
    };
    let arch = normalize_arch(raw_arch)
        .ok_or_else(|| DeployError::Detect(format!("unknown CPU {raw_arch:?}")))?;
    if home.is_empty() || home.eq_ignore_ascii_case("%USERPROFILE%") {
        return Err(DeployError::Detect("USERPROFILE is not set".into()));
    }
    Ok((arch, home.clone()))
}

/// Detect OS, CPU, shell and home directory of the remote host.
pub async fn detect<P: Prompter>(s: &SshSession<P>) -> Result<Remote, DeployError> {
    let posix = s.exec(&posix_detect_command()).await?;
    if let Some((os, arch, home)) = parse_posix_detect(&posix.stdout_str())? {
        return Ok(Remote {
            os,
            arch,
            shell: Shell::Posix,
            home,
        });
    }
    // Windows: cmd expands %OS%, PowerShell prints it verbatim.
    let shell = match s.exec("echo %OS%").await?.stdout_str().trim() {
        "Windows_NT" => Shell::Cmd,
        "%OS%" => Shell::PowerShell,
        other => {
            return Err(DeployError::Detect(format!(
                "neither a POSIX shell nor Windows (%OS%: {other:?})"
            )));
        }
    };
    let out = s.exec(&windows_detect_command(shell)).await?;
    let (arch, home) = parse_windows_detect(&out.stdout_str())?;
    Ok(Remote {
        os: Os::Windows,
        arch,
        shell,
        home,
    })
}

fn upload_err(e: impl fmt::Display) -> DeployError {
    DeployError::Upload(e.to_string())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Upload `bytes` to `<login dir>/<dir>/bin/<exe>` via SFTP (paths are
/// relative to the login directory, which is home on OpenSSH servers).
/// The file is written to `<exe>.upload`, its SHA-256 is checked on the
/// host (B18), and it is swapped in with the old binary moved aside
/// rather than deleted (B19).
async fn upload<P: Prompter>(
    s: &SshSession<P>,
    remote: &Remote,
    dir: &str,
    bytes: &[u8],
) -> Result<(), DeployError> {
    let sftp = s.sftp().await?;
    let bin_dir = format!("{dir}/bin");
    let mut path = String::new();
    for part in bin_dir.split('/') {
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(part);
        if !sftp.try_exists(path.clone()).await.map_err(upload_err)? {
            sftp.create_dir(path.clone()).await.map_err(upload_err)?;
        }
    }
    let target = format!("{bin_dir}/{}", remote.exe_name());
    let tmp = format!("{target}.upload");
    let mut file = sftp.create(tmp.clone()).await.map_err(upload_err)?;
    file.write_all(bytes).await.map_err(upload_err)?;
    file.shutdown().await.map_err(upload_err)?;
    if remote.os != Os::Windows {
        let attrs = russh_sftp::protocol::FileAttributes {
            permissions: Some(0o755),
            ..Default::default()
        };
        sftp.set_metadata(tmp.clone(), attrs)
            .await
            .map_err(upload_err)?;
    }
    let tmp_abs = format!("{}.upload", remote.probe_path(dir));
    let written = parse_hash(&s.exec(&remote.hash_command(&tmp_abs)).await?.stdout_str());
    if written.as_deref() != Some(sha256_hex(bytes).as_str()) {
        let _ = sftp.remove_file(tmp).await;
        return Err(DeployError::Upload(format!(
            "uploaded file does not match (SHA-256 {})",
            written.unwrap_or_else(|| "unknown".into())
        )));
    }
    swap_in(&SftpFiles(&sftp), &bin_dir, remote.exe_name(), now_ms())
        .await
        .map_err(|e| DeployError::Upload(format!("replace {target}: {e}")))
}

/// Make sure the right probe is on the host, then (optionally) install
/// agent hooks. Uploads only when the remote SHA-256 differs.
pub async fn deploy<P: Prompter>(
    s: &SshSession<P>,
    store: &ProbeStore,
    opts: &DeployOptions,
) -> Result<DeployReport, DeployError> {
    let remote = detect(s).await?;
    let target = remote.target().ok_or_else(|| {
        DeployError::Detect(format!("unsupported {:?} {}", remote.os, remote.arch))
    })?;
    let local = store.binary(target);
    let bytes = std::fs::read(&local).map_err(|_| DeployError::NoBinary {
        target: target.to_owned(),
        path: local.clone(),
    })?;
    let probe_path = remote.probe_path(&opts.dir);
    let remote_hash = parse_hash(
        &s.exec(&remote.hash_command(&probe_path))
            .await?
            .stdout_str(),
    );
    let uploaded = remote_hash.as_deref() != Some(sha256_hex(&bytes).as_str());
    if uploaded {
        if remote_hash.is_some() {
            // Best effort: an older probe may not know `stop`.
            let _ = s
                .exec(&remote.invoke(&probe_path, &stop_args(&opts.client)))
                .await;
        }
        upload(s, &remote, &opts.dir, &bytes).await?;
    }
    let hooks = if opts.install_hooks {
        let out = s.exec(&remote.invoke(&probe_path, "install-hooks")).await?;
        hooks_result(&out)?
    } else {
        Vec::new()
    };
    Ok(DeployReport {
        remote,
        probe_path,
        uploaded,
        hooks,
    })
}
```

- [ ] **Step 5: `crates/mai-core/src/connect.rs`（完整内容）**

```rust
//! The real `Connector`: SSH hosts get the probe deployed over SSH and
//! `serve` started on an exec channel; the local host gets the probe
//! copied into the home directory and `serve` started as a child process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::BufReader;

use crate::deploy::{
    DeployError, DeployOptions, HookResult, ProbeStore, Remote, Shell, deploy, detect,
    hooks_result, normalize_arch, serve_argv, sha256_hex, stop_argv,
};
use crate::host::{Connector, HostConfig, HostKind, OpenError, Opened, Problem};
use crate::link::ProbeIo;
use crate::ssh::auth::{Prompter, SecretStore};
use crate::ssh::client::{ConnectOptions, ExecOutput, SshError, connect};
use crate::ssh::config::{HostSpec, parse_config, resolve};
use crate::stderr::{StderrTail, collect as collect_stderr};
use crate::swap::{LocalFiles, swap_in};
use crate::terminals::SystemTerminals;

/// Locale requested for `serve` so zellij output is UTF-8.
const LOCALE: &str = "en_US.UTF-8";

/// Map an SSH failure to retry-or-ask-the-user.
pub fn ssh_open_error(e: SshError) -> OpenError {
    match e {
        SshError::Connect(m) | SshError::Channel(m) => OpenError::Retry(m),
        SshError::Auth(m) => OpenError::NeedsUser(Problem::Auth(m)),
        SshError::HostKeyRejected { .. } => OpenError::NeedsUser(Problem::HostKeyRejected),
        SshError::HostKeyChanged { file, line, .. } => {
            OpenError::NeedsUser(Problem::HostKeyChanged { file, line })
        }
        SshError::HostKeyRevoked { file, line, .. } => {
            OpenError::NeedsUser(Problem::HostKeyRevoked { file, line })
        }
    }
}

/// Map a deployment failure to retry-or-ask-the-user.
pub fn deploy_open_error(e: DeployError) -> OpenError {
    match e {
        DeployError::Ssh(e) => ssh_open_error(e),
        other => OpenError::NeedsUser(Problem::Deploy(other.to_string())),
    }
}

fn hooks_outcome(out: &ExecOutput) -> Result<Vec<HookResult>, String> {
    hooks_result(out).map_err(|e| e.to_string())
}

/// Remote-style description of this machine, for choosing the probe
/// binary and building command lines.
pub fn local_remote(home: &Path) -> Option<Remote> {
    use crate::deploy::Os;
    let os = match std::env::consts::OS {
        "linux" => Os::Linux,
        "macos" => Os::MacOs,
        "windows" => Os::Windows,
        _ => return None,
    };
    Some(Remote {
        os,
        arch: normalize_arch(std::env::consts::ARCH)?,
        shell: if os == Os::Windows {
            Shell::Cmd
        } else {
            Shell::Posix
        },
        home: home.to_string_lossy().into_owned(),
    })
}

/// Put `bytes` at `exe` unless it already has them. `stop` runs first
/// when an existing, different binary is replaced; the old binary is then
/// moved aside rather than deleted (a running probe locks its file on
/// Windows), see `swap::swap_in`. Returns whether the file was written.
pub async fn place_binary(
    exe: &Path,
    bytes: &[u8],
    stop: impl Future<Output = ()>,
) -> std::io::Result<bool> {
    let existing = tokio::fs::read(exe).await.ok();
    if existing.as_deref().map(sha256_hex) == Some(sha256_hex(bytes)) {
        return Ok(false);
    }
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else {
        return Err(std::io::Error::other("probe path has no directory"));
    };
    tokio::fs::create_dir_all(dir).await?;
    let mut tmp = exe.as_os_str().to_owned();
    tmp.push(".upload");
    let tmp = PathBuf::from(tmp);
    tokio::fs::write(&tmp, bytes).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).await?;
    }
    if existing.is_some() {
        stop.await;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    swap_in(
        &LocalFiles,
        &dir.to_string_lossy(),
        &name.to_string_lossy(),
        stamp,
    )
    .await?;
    Ok(true)
}

#[cfg(windows)]
fn no_window(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    cmd.creation_flags(0x0800_0000)
}

#[cfg(not(windows))]
fn no_window(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    cmd
}

/// Run `exe` with `argv` and collect its output.
async fn run_local(exe: &Path, argv: &[String]) -> std::io::Result<ExecOutput> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(argv).stdin(Stdio::null());
    let out = no_window(&mut cmd).output().await?;
    Ok(ExecOutput {
        status: out.status.code().and_then(|c| u32::try_from(c).ok()),
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// Everything needed to reach hosts and start their probes.
pub struct SystemConnector<P, S> {
    pub opts: ConnectOptions,
    pub prompter: Arc<P>,
    pub secrets: Arc<S>,
    pub probes: ProbeStore,
    /// `~/.ssh/config`; read on every connect so edits take effect.
    pub ssh_config: Option<PathBuf>,
    /// Local home: default keys, and where the local probe is installed.
    pub home: PathBuf,
    /// User for targets that name none.
    pub default_user: String,
    /// This app installation's id (see `mai-probe serve --client`).
    pub client: String,
    /// Deploy dir under the home directory (`.mai`).
    pub dir: String,
    /// Install agent hooks after deploying.
    pub install_hooks: bool,
    gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl<P: Prompter, S: SecretStore> SystemConnector<P, S> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        opts: ConnectOptions,
        prompter: Arc<P>,
        secrets: Arc<S>,
        probes: ProbeStore,
        ssh_config: Option<PathBuf>,
        home: PathBuf,
        default_user: String,
        client: String,
    ) -> Self {
        Self {
            opts,
            prompter,
            secrets,
            probes,
            ssh_config,
            home,
            default_user,
            client,
            dir: ".mai".to_owned(),
            install_hooks: true,
            gates: Mutex::new(HashMap::new()),
        }
    }

    /// Lock held while connecting to and deploying on host `id`, so
    /// host-key prompts and uploads for one host never overlap.
    pub fn gate(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(|p| p.into_inner());
        gates.entry(id.to_owned()).or_default().clone()
    }

    fn spec(&self, target: &str) -> Result<HostSpec, OpenError> {
        let text = match &self.ssh_config {
            None => String::new(),
            Some(p) => match std::fs::read_to_string(p) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    return Err(OpenError::NeedsUser(Problem::Config(format!(
                        "{}: {e}",
                        p.display()
                    ))));
                }
            },
        };
        let config = parse_config(&text)
            .map_err(|e| OpenError::NeedsUser(Problem::Config(e.to_string())))?;
        resolve(&config, target, &self.default_user, &self.home)
            .map_err(|e| OpenError::NeedsUser(Problem::Config(e.to_string())))
    }

    async fn open_ssh(&self, host: &HostConfig, target: &str) -> Result<Opened, OpenError> {
        let spec = self.spec(target)?;
        let gate = self.gate(&host.id);
        let guard = gate.lock().await;
        let session = connect(&spec, &self.opts, self.prompter.clone(), &*self.secrets)
            .await
            .map_err(ssh_open_error)?;
        let opts = DeployOptions {
            dir: self.dir.clone(),
            install_hooks: false,
            client: self.client.clone(),
        };
        let report = deploy(&session, &self.probes, &opts)
            .await
            .map_err(deploy_open_error)?;
        let remote = &report.remote;
        let hooks = if self.install_hooks {
            let cmd = remote.invoke(&report.probe_path, "install-hooks");
            let out = session.exec(&cmd).await.map_err(ssh_open_error)?;
            hooks_outcome(&out)
        } else {
            Ok(Vec::new())
        };
        drop(guard);
        let args = remote.serve_args(&self.client, host.zellij.as_deref());
        let env = [("LANG", LOCALE), ("LC_CTYPE", LOCALE)];
        let stderr = Arc::new(StderrTail::default());
        let (r, w) = session
            .open_exec_split(
                &remote.invoke(&report.probe_path, &args),
                &env,
                stderr.clone(),
            )
            .await
            .map_err(ssh_open_error)?;
        Ok(Opened {
            io: ProbeIo {
                reader: Box::new(BufReader::new(r)),
                writer: w,
            },
            hooks,
            keep: Box::new(session),
            stderr,
        })
    }

    async fn open_local(&self, host: &HostConfig) -> Result<Opened, OpenError> {
        let deploy_err = |m: String| OpenError::NeedsUser(Problem::Deploy(m));
        let remote = local_remote(&self.home)
            .ok_or_else(|| deploy_err("unsupported local platform".into()))?;
        let target = remote
            .target()
            .ok_or_else(|| deploy_err("unsupported local platform".into()))?;
        let bin = self.probes.binary(target);
        let bytes =
            std::fs::read(&bin).map_err(|e| deploy_err(format!("{}: {e}", bin.display())))?;
        let exe = PathBuf::from(remote.probe_path(&self.dir));
        let gate = self.gate(&host.id);
        let guard = gate.lock().await;
        let stop = async {
            let _ = run_local(&exe, &stop_argv(&self.client)).await;
        };
        place_binary(&exe, &bytes, stop)
            .await
            .map_err(|e| deploy_err(format!("{}: {e}", exe.display())))?;
        let hooks = if self.install_hooks {
            match run_local(&exe, &["install-hooks".to_owned()]).await {
                Ok(out) => hooks_outcome(&out),
                Err(e) => Err(e.to_string()),
            }
        } else {
            Ok(Vec::new())
        };
        drop(guard);
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.args(serve_argv(&self.client, host.zellij.as_deref()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = no_window(&mut cmd)
            .spawn()
            .map_err(|e| OpenError::Retry(format!("start {}: {e}", exe.display())))?;
        let (Some(out), Some(input), Some(err)) =
            (child.stdout.take(), child.stdin.take(), child.stderr.take())
        else {
            return Err(OpenError::Retry("probe pipes unavailable".into()));
        };
        let stderr = Arc::new(StderrTail::default());
        tokio::spawn(collect_stderr(err, stderr.clone()));
        Ok(Opened {
            io: ProbeIo {
                reader: Box::new(BufReader::new(out)),
                writer: Box::new(input),
            },
            hooks,
            keep: Box::new(child),
            stderr,
        })
    }
}

impl<P: Prompter, S: SecretStore> SystemConnector<P, S> {
    /// A second SSH session for terminals. Connecting holds the host's
    /// gate, so it never prompts for a host key at the same time as the
    /// probe connection.
    async fn open_ssh_terminals(
        &self,
        host: &HostConfig,
        target: &str,
    ) -> Result<SystemTerminals<P>, OpenError> {
        let spec = self.spec(target)?;
        let gate = self.gate(&host.id);
        let _guard = gate.lock().await;
        let session = connect(&spec, &self.opts, self.prompter.clone(), &*self.secrets)
            .await
            .map_err(ssh_open_error)?;
        let remote = detect(&session).await.map_err(deploy_open_error)?;
        Ok(SystemTerminals::Ssh { session, remote })
    }
}

impl<P: Prompter, S: SecretStore> Connector for SystemConnector<P, S> {
    type Terminals = SystemTerminals<P>;

    async fn open(&self, host: &HostConfig) -> Result<Opened, OpenError> {
        match &host.kind {
            HostKind::Local => self.open_local(host).await,
            HostKind::Ssh { target } => self.open_ssh(host, target).await,
        }
    }

    async fn open_terminals(&self, host: &HostConfig) -> Result<SystemTerminals<P>, OpenError> {
        match &host.kind {
            HostKind::Local => Ok(SystemTerminals::Local),
            HostKind::Ssh { target } => self.open_ssh_terminals(host, target).await,
        }
    }
}
```

- [ ] **Step 6: `crates/mai-core/src/lib.rs`（完整内容）**

```rust
//! UI-independent core of the multi-ai app.

pub mod connect;
pub mod deploy;
pub mod host;
pub mod link;
pub mod manager;
pub mod monitor;
pub mod pty;
pub mod ssh;
pub mod stderr;
pub mod swap;
pub mod term;
pub mod terminals;
pub mod tracker;
```

- [ ] **Step 7: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: `swap` 4 个（非 Windows 为 3 个）、`deploy` 17 个测试通过，全部 241 个；
clippy 无警告。

- [ ] **Step 8: 提交**

```bash
git add crates/mai-core
git commit -m "feat(core): detection sentinels, upload hash check, swap probe in by moving the old one aside

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: 提示（notes）交给 UI

**Files:**

- Modify: `crates/mai-core/src/host.rs`、`crates/mai-core/src/monitor.rs`、
  `crates/mai-core/src/connect.rs`
- Test: `crates/mai-core/tests/hosts.rs`、`crates/mai-core/tests/monitor.rs`

**Interfaces:**

- Consumes: `config_warnings`（Task 2）、`SshSession::notes`（Task 3）、
  Task 4 的 `connect.rs`。
- Produces:

  ```rust
  // host
  pub struct Opened { .., pub notes: Vec<String>, .. }
  HostEvent::Notes(Vec<String>)            // Hello、Hooks 之后；为空时不发
  // monitor
  Update::Notes { id: String, notes: Vec<String> }
  ```

  SSH 连接的提示 = `config_warnings(ssh config 文本)` + `session.notes()`；
  本机连接没有提示。

- [ ] **Step 1: 写失败测试**

`crates/mai-core/tests/hosts.rs`（完整内容）：

```rust
//! Host tasks and the manager against a scripted connector and in-memory
//! probes. Time is paused: backoff waits and timeouts pass instantly.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use mai_core::host::{
    Backoff, ConnState, Connector, HostCommand, HostConfig, HostEvent, HostKind, HostState,
    OpenError, Opened, Problem, STABLE_AFTER, host_state, run_host,
};
use mai_core::link::ProbeIo;
use mai_core::manager::HostManager;
use mai_core::monitor::Update;
use mai_core::pty::{PtyEnds, PtyIn, PtyIo, TermSize, pty_channels};
use mai_core::stderr::StderrTail;
use mai_core::term::{AttachSpec, TermError, TermEvent, TermTransport};
use mai_core::tracker::{AgentKey, TrackerConfig};
use mai_protocol::{
    AgentEvent, AgentState, AppMsg, EventSource, PROTOCOL_VERSION, PaneRef, ProbeMsg, decode_line,
    encode_line,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, Lines};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;
use tokio::time::timeout;

/// The probe's ends of one started `serve`.
struct FakeProbe {
    out: DuplexStream,
    input: Lines<BufReader<DuplexStream>>,
}

impl FakeProbe {
    async fn send(&mut self, msg: &ProbeMsg) {
        let line = encode_line(msg).unwrap();
        self.out.write_all(line.as_bytes()).await.unwrap();
    }

    async fn hello(&mut self, version: u32) {
        self.hello_with(version, None).await;
    }

    async fn hello_with(&mut self, version: u32, zellij: Option<&str>) {
        self.send(&ProbeMsg::Hello {
            protocol_version: version,
            probe_version: "0.1.0".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            zellij_path: zellij.map(str::to_owned),
            zellij_version: None,
        })
        .await;
    }

    async fn heard(&mut self) -> AppMsg {
        let line = timeout(Duration::from_secs(60), self.input.next_line())
            .await
            .expect("app wrote a line")
            .unwrap()
            .unwrap();
        decode_line(&line).unwrap()
    }
}

enum Step {
    Fail(OpenError),
    Probe,
}

/// One terminal attach: the zellij binary used, what was attached, and
/// the PTY ends to drive it.
struct Attach {
    zellij: Option<String>,
    spec: AttachSpec,
    ends: PtyEnds,
}

/// Terminal connection of `Scripted`: every attach succeeds.
struct ScriptedTerms {
    attaches: UnboundedSender<Attach>,
}

impl TermTransport for ScriptedTerms {
    async fn attach(
        &mut self,
        zellij: Option<&str>,
        spec: &AttachSpec,
    ) -> Result<PtyIo, OpenError> {
        let (io, ends) = pty_channels();
        let _ = self.attaches.send(Attach {
            zellij: zellij.map(str::to_owned),
            spec: spec.clone(),
            ends,
        });
        Ok(io)
    }
}

/// Replays `steps` in order; hangs once they run out.
struct Scripted {
    steps: Mutex<VecDeque<Step>>,
    opens: AtomicUsize,
    probes: UnboundedSender<FakeProbe>,
    attaches: UnboundedSender<Attach>,
    attaches_rx: Mutex<Option<UnboundedReceiver<Attach>>>,
    /// Stderr of every probe this connector starts.
    stderr: Arc<StderrTail>,
    /// Notes every probe this connector starts reports.
    notes: Mutex<Vec<String>>,
}

impl Scripted {
    fn new(steps: Vec<Step>) -> (Arc<Self>, UnboundedReceiver<FakeProbe>) {
        let (probes, rx) = mpsc::unbounded_channel();
        let (attaches, attaches_rx) = mpsc::unbounded_channel();
        let c = Self {
            steps: Mutex::new(steps.into()),
            opens: AtomicUsize::new(0),
            probes,
            attaches,
            attaches_rx: Mutex::new(Some(attaches_rx)),
            stderr: Arc::default(),
            notes: Mutex::default(),
        };
        (Arc::new(c), rx)
    }

    fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }

    /// Terminal attaches made through this connector.
    fn attaches(&self) -> UnboundedReceiver<Attach> {
        self.attaches_rx.lock().unwrap().take().expect("taken once")
    }
}

impl Connector for Scripted {
    type Terminals = ScriptedTerms;

    async fn open_terminals(&self, _host: &HostConfig) -> Result<ScriptedTerms, OpenError> {
        Ok(ScriptedTerms {
            attaches: self.attaches.clone(),
        })
    }

    async fn open(&self, _host: &HostConfig) -> Result<Opened, OpenError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let step = self.steps.lock().unwrap().pop_front();
        match step {
            None => std::future::pending().await,
            Some(Step::Fail(e)) => Err(e),
            Some(Step::Probe) => {
                let (app_read, out) = tokio::io::duplex(64 * 1024);
                let (app_write, probe_in) = tokio::io::duplex(64 * 1024);
                let probe = FakeProbe {
                    out,
                    input: BufReader::new(probe_in).lines(),
                };
                self.probes.send(probe).unwrap();
                Ok(Opened {
                    io: ProbeIo {
                        reader: Box::new(BufReader::new(app_read)),
                        writer: Box::new(app_write),
                    },
                    hooks: Ok(Vec::new()),
                    keep: Box::new(()),
                    notes: self.notes.lock().unwrap().clone(),
                    stderr: self.stderr.clone(),
                })
            }
        }
    }
}

fn cfg(id: &str) -> HostConfig {
    HostConfig {
        id: id.into(),
        kind: HostKind::Local,
        zellij: None,
    }
}

fn retry(m: &str) -> Step {
    Step::Fail(OpenError::Retry(m.into()))
}

struct Harness {
    events: UnboundedReceiver<(String, HostEvent)>,
    cmds: UnboundedSender<HostCommand>,
    probes: UnboundedReceiver<FakeProbe>,
    task: JoinHandle<()>,
}

impl Harness {
    fn start(connector: Arc<Scripted>, probes: UnboundedReceiver<FakeProbe>) -> Self {
        let (ev_tx, events) = mpsc::unbounded_channel();
        let (cmds, cmd_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run_host(cfg("h"), connector, ev_tx, cmd_rx));
        Self {
            events,
            cmds,
            probes,
            task,
        }
    }

    async fn event(&mut self) -> HostEvent {
        let (id, ev) = timeout(Duration::from_secs(600), self.events.recv())
            .await
            .expect("host event")
            .expect("host task alive");
        assert_eq!(id, "h");
        ev
    }

    /// Next connection state, skipping other events.
    async fn state(&mut self) -> ConnState {
        loop {
            if let HostEvent::Probe(s) = self.event().await {
                return s;
            }
        }
    }

    async fn probe(&mut self) -> FakeProbe {
        self.probes.recv().await.expect("probe started")
    }
}

fn retrying(s: &ConnState) -> Option<Duration> {
    match s {
        ConnState::Retrying { retry_in, .. } => Some(*retry_in),
        _ => None,
    }
}

fn hook_event(offset: u64, state: AgentState) -> ProbeMsg {
    ProbeMsg::AgentEvent(AgentEvent {
        pane: PaneRef {
            session: "w".into(),
            pane_id: 1,
        },
        agent: "claude".into(),
        source: EventSource::Hook,
        state,
        message: None,
        ts_ms: offset,
        spool_offset: Some(offset),
    })
}

#[test]
fn backoff_doubles_to_a_minute_and_resets() {
    let mut b = Backoff::default();
    let secs: Vec<u64> = (0..8).map(|_| b.next_delay().as_secs()).collect();
    assert_eq!(secs, vec![1, 2, 4, 8, 16, 32, 60, 60]);
    b.reset();
    assert_eq!(b.next_delay(), Duration::from_secs(1));
}

#[test]
fn host_state_combines_connections() {
    let down = ConnState::Retrying {
        retry_in: Duration::from_secs(1),
        reason: "x".into(),
    };
    let auth = ConnState::NeedsUser(Problem::HostKeyRejected);
    let deploy = ConnState::NeedsUser(Problem::Deploy("x".into()));
    assert_eq!(host_state(&ConnState::Up, None), HostState::Online);
    assert_eq!(
        host_state(&ConnState::Up, Some(&ConnState::Up)),
        HostState::Online
    );
    assert_eq!(host_state(&ConnState::Up, Some(&down)), HostState::Degraded);
    assert_eq!(
        host_state(&deploy, Some(&ConnState::Up)),
        HostState::Degraded
    );
    assert_eq!(host_state(&auth, None), HostState::AuthRequired);
    assert_eq!(
        host_state(&ConnState::Connecting, None),
        HostState::Connecting
    );
    assert_eq!(host_state(&down, None), HostState::Offline);
    assert_eq!(host_state(&deploy, None), HostState::Offline);
}

#[tokio::test(start_paused = true)]
async fn connected_probe_relays_messages_and_acks_hook_events() {
    let (c, probes) = Scripted::new(vec![Step::Probe]);
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    let mut probe = h.probe().await;
    probe.hello(PROTOCOL_VERSION).await;
    assert!(matches!(h.event().await, HostEvent::Hello(i) if i.os == "linux"));
    assert_eq!(h.event().await, HostEvent::Hooks(Ok(Vec::new())));
    assert_eq!(h.event().await, HostEvent::Probe(ConnState::Up));

    probe.send(&hook_event(42, AgentState::Done)).await;
    assert!(matches!(
        h.event().await,
        HostEvent::Msg(ProbeMsg::AgentEvent(_))
    ));
    assert_eq!(probe.heard().await, AppMsg::Ack { spool_offset: 42 });

    h.cmds
        .send(HostCommand::Send(AppMsg::Focus {
            session: "w".into(),
            pane_id: 1,
            tab_id: 0,
        }))
        .unwrap();
    assert!(matches!(
        probe.heard().await,
        AppMsg::Focus { pane_id: 1, .. }
    ));
}

#[tokio::test(start_paused = true)]
async fn transient_failures_back_off_then_connect() {
    let (c, probes) = Scripted::new(vec![retry("net down"), retry("net down"), Step::Probe]);
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    let s = h.state().await;
    assert_eq!(retrying(&s), Some(Duration::from_secs(1)));
    assert!(matches!(&s, ConnState::Retrying { reason, .. } if reason == "net down"));
    assert_eq!(h.state().await, ConnState::Connecting);
    assert_eq!(retrying(&h.state().await), Some(Duration::from_secs(2)));
    assert_eq!(h.state().await, ConnState::Connecting);
    h.probe().await.hello(PROTOCOL_VERSION).await;
    assert_eq!(h.state().await, ConnState::Up);
}

#[tokio::test(start_paused = true)]
async fn problem_needing_the_user_waits_for_retry() {
    let (c, probes) = Scripted::new(vec![
        Step::Fail(OpenError::NeedsUser(Problem::Auth("denied".into()))),
        retry("later"),
    ]);
    let mut h = Harness::start(c.clone(), probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    assert_eq!(
        h.state().await,
        ConnState::NeedsUser(Problem::Auth("denied".into()))
    );
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(c.opens(), 1, "no automatic retry");

    h.cmds
        .send(HostCommand::Send(AppMsg::Ack { spool_offset: 1 }))
        .unwrap();
    assert_eq!(
        h.event().await,
        HostEvent::Dropped(AppMsg::Ack { spool_offset: 1 })
    );
    h.cmds.send(HostCommand::Retry).unwrap();
    assert_eq!(h.state().await, ConnState::Connecting);
    assert_eq!(retrying(&h.state().await), Some(Duration::from_secs(1)));
    assert_eq!(c.opens(), 2);
}

#[tokio::test(start_paused = true)]
async fn retry_command_skips_the_backoff_wait() {
    let (c, probes) = Scripted::new(vec![retry("a"), retry("b"), retry("c")]);
    let mut h = Harness::start(c.clone(), probes);
    for _ in 0..4 {
        h.state().await;
    }
    // Now waiting 2 s before the third attempt; Retry starts it at once.
    let before = tokio::time::Instant::now();
    h.cmds.send(HostCommand::Retry).unwrap();
    assert_eq!(h.state().await, ConnState::Connecting);
    assert!(before.elapsed() < Duration::from_secs(2));
}

#[tokio::test(start_paused = true)]
async fn probe_exit_reconnects_and_short_lived_probe_keeps_backing_off() {
    let (c, probes) = Scripted::new(vec![retry("x"), Step::Probe, Step::Probe]);
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    assert_eq!(retrying(&h.state().await), Some(Duration::from_secs(1)));
    assert_eq!(h.state().await, ConnState::Connecting);
    let mut probe = h.probe().await;
    probe.hello(PROTOCOL_VERSION).await;
    assert_eq!(h.state().await, ConnState::Up);
    drop(probe);
    let s = h.state().await;
    assert_eq!(retrying(&s), Some(Duration::from_secs(2)), "{s:?}");
    assert!(matches!(&s, ConnState::Retrying { reason, .. } if reason.contains("closed")));
    assert_eq!(h.state().await, ConnState::Connecting);
}

#[tokio::test(start_paused = true)]
async fn stable_probe_resets_backoff_when_it_drops() {
    let (c, probes) = Scripted::new(vec![retry("x"), retry("x"), Step::Probe]);
    let mut h = Harness::start(c, probes);
    for _ in 0..5 {
        h.state().await;
    }
    let mut probe = h.probe().await;
    probe.hello(PROTOCOL_VERSION).await;
    assert_eq!(h.state().await, ConnState::Up);
    let mut waited = Duration::ZERO;
    while waited <= STABLE_AFTER {
        probe.send(&ProbeMsg::Heartbeat { ts_ms: 1 }).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        waited += Duration::from_secs(5);
    }
    drop(probe);
    assert_eq!(retrying(&h.state().await), Some(Duration::from_secs(1)));
}

#[tokio::test(start_paused = true)]
async fn other_protocol_version_needs_the_user() {
    let (c, probes) = Scripted::new(vec![Step::Probe]);
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    h.probe().await.hello(PROTOCOL_VERSION + 1).await;
    assert_eq!(
        h.state().await,
        ConnState::NeedsUser(Problem::Protocol {
            probe: PROTOCOL_VERSION + 1,
            app: PROTOCOL_VERSION
        })
    );
}

#[tokio::test(start_paused = true)]
async fn stop_ends_the_task_even_while_connecting() {
    let (c, probes) = Scripted::new(vec![]);
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    h.cmds.send(HostCommand::Stop).unwrap();
    timeout(Duration::from_secs(5), h.task)
        .await
        .expect("task ended")
        .unwrap();
}

async fn next_update(rx: &mut UnboundedReceiver<Update>) -> Update {
    timeout(Duration::from_secs(600), rx.recv())
        .await
        .expect("update")
        .expect("monitor alive")
}

/// Skip updates until `f` matches one.
async fn until<T>(rx: &mut UnboundedReceiver<Update>, f: impl Fn(&Update) -> Option<T>) -> T {
    loop {
        if let Some(t) = f(&next_update(rx).await) {
            return t;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn manager_turns_probe_events_into_updates() {
    let (c, mut probes) = Scripted::new(vec![Step::Probe]);
    let (mut mgr, mut rx) = HostManager::start(c, TrackerConfig::default());
    assert!(mgr.add_host(cfg("h")));
    assert!(!mgr.add_host(cfg("h")), "duplicate id");
    assert_eq!(mgr.host_ids(), vec!["h".to_owned()]);

    let mut probe = probes.recv().await.unwrap();
    probe.hello(PROTOCOL_VERSION).await;
    let state = until(&mut rx, |u| match u {
        Update::Host { state, .. } if *state == HostState::Online => Some(*state),
        _ => None,
    })
    .await;
    assert_eq!(state, HostState::Online);

    probe.send(&hook_event(7, AgentState::NeedsInput)).await;
    let alert = until(&mut rx, |u| match u {
        Update::Alert(a) => Some(a.clone()),
        _ => None,
    })
    .await;
    assert_eq!(alert.state, AgentState::NeedsInput);
    assert_eq!(probe.heard().await, AppMsg::Ack { spool_offset: 7 });

    mgr.acknowledge(AgentKey {
        host_id: "h".into(),
        session: "w".into(),
        pane_id: 1,
    });
    let acked = until(&mut rx, |u| match u {
        Update::Agent(r) => Some(r.acknowledged),
        _ => None,
    })
    .await;
    assert!(acked);

    assert!(mgr.send(
        "h",
        AppMsg::SendText {
            session: "w".into(),
            pane_id: 1,
            text: "yes".into()
        }
    ));
    assert!(matches!(probe.heard().await, AppMsg::SendText { text, .. } if text == "yes"));
    assert!(!mgr.send("nope", AppMsg::Ack { spool_offset: 1 }));
}

#[tokio::test(start_paused = true)]
async fn removed_host_can_be_added_again() {
    let (c, mut probes) = Scripted::new(vec![Step::Probe, Step::Probe]);
    let (mut mgr, mut rx) = HostManager::start(c.clone(), TrackerConfig::default());
    mgr.add_host(cfg("h"));
    let _first = probes.recv().await.unwrap();
    assert!(mgr.remove_host("h"));
    assert!(!mgr.remove_host("h"));
    assert!(mgr.host_ids().is_empty());

    assert!(mgr.add_host(cfg("h")));
    let mut second = probes.recv().await.unwrap();
    second.hello(PROTOCOL_VERSION).await;
    let state = until(&mut rx, |u| match u {
        Update::Host { state, .. } if *state == HostState::Online => Some(*state),
        _ => None,
    })
    .await;
    assert_eq!(state, HostState::Online);
    assert_eq!(c.opens(), 2);
}

#[tokio::test(start_paused = true)]
async fn dropping_the_manager_ends_the_update_stream() {
    let (c, _probes) = Scripted::new(vec![]);
    let (mut mgr, mut rx) = HostManager::start(c, TrackerConfig::default());
    mgr.add_host(cfg("h"));
    drop(mgr);
    loop {
        match timeout(Duration::from_secs(60), rx.recv()).await {
            Ok(Some(_)) => continue,
            Ok(None) => break,
            Err(_) => panic!("update stream did not end"),
        }
    }
}

#[tokio::test(start_paused = true)]
async fn terminal_uses_the_probe_zellij_and_joins_the_host_state() {
    let (c, mut probes) = Scripted::new(vec![Step::Probe]);
    let mut attaches = c.attaches();
    let (mut mgr, mut rx) = HostManager::start(c, TrackerConfig::default());
    mgr.add_host(cfg("h"));
    let size = TermSize { cols: 80, rows: 24 };
    assert_eq!(
        mgr.open_terminal("nope", "w", false, size).await.err(),
        Some(TermError::NoHost)
    );

    let mut probe = probes.recv().await.unwrap();
    probe
        .hello_with(PROTOCOL_VERSION, Some("/usr/bin/zellij"))
        .await;
    until(&mut rx, |u| match u {
        Update::Hello { .. } => Some(()),
        _ => None,
    })
    .await;

    let mut term = mgr.open_terminal("h", "work", false, size).await.unwrap();
    assert_eq!(term.recv().await, Some(TermEvent::Attached));
    let mut pty = attaches.recv().await.unwrap();
    assert_eq!(pty.zellij.as_deref(), Some("/usr/bin/zellij"));
    assert_eq!(pty.spec.session, "work");
    let term_up = until(&mut rx, |u| match u {
        Update::Host {
            term: Some(t),
            state,
            ..
        } => Some((t.clone(), *state)),
        _ => None,
    })
    .await;
    assert_eq!(term_up, (ConnState::Up, HostState::Online));

    term.write(b"q".to_vec());
    assert_eq!(
        pty.ends.input.recv().await,
        Some(PtyIn::Data(b"q".to_vec()))
    );
    drop(term);
    assert_eq!(pty.ends.input.recv().await, Some(PtyIn::Close));
    // The last terminal is gone: the terminal connection is closed.
    until(&mut rx, |u| match u {
        Update::Host { term: None, .. } => Some(()),
        _ => None,
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn probe_stderr_explains_why_it_stopped() {
    let (c, probes) = Scripted::new(vec![Step::Probe]);
    let mut h = Harness::start(c.clone(), probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    let probe = h.probe().await;
    c.stderr
        .push(b"mai-probe serve: rules.toml: invalid regex\n");
    c.stderr.finish();
    drop(probe);
    match h.state().await {
        ConnState::Retrying { reason, .. } => {
            assert!(reason.contains("closed"), "{reason}");
            assert!(
                reason.contains("probe stderr: mai-probe serve: rules.toml: invalid regex"),
                "{reason}"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn dropping_the_manager_ends_open_terminals() {
    let (c, _probes) = Scripted::new(vec![]);
    let mut attaches = c.attaches();
    let (mut mgr, _rx) = HostManager::start(c, TrackerConfig::default());
    mgr.add_host(cfg("h"));
    let size = TermSize { cols: 80, rows: 24 };
    let mut term = mgr.open_terminal("h", "work", false, size).await.unwrap();
    assert_eq!(term.recv().await, Some(TermEvent::Attached));
    let mut pty = attaches.recv().await.unwrap();
    drop(mgr);
    assert_eq!(pty.ends.input.recv().await, Some(PtyIn::Close));
    let end = timeout(Duration::from_secs(60), term.recv())
        .await
        .expect("terminal stream ended");
    assert_eq!(end, None);
}

#[tokio::test(start_paused = true)]
async fn connect_notes_are_reported_after_hello() {
    let (c, probes) = Scripted::new(vec![Step::Probe]);
    c.notes
        .lock()
        .unwrap()
        .push("ssh config line 3: Match blocks are not supported".into());
    let mut h = Harness::start(c, probes);
    assert_eq!(h.state().await, ConnState::Connecting);
    h.probe().await.hello(PROTOCOL_VERSION).await;
    assert!(matches!(h.event().await, HostEvent::Hello(_)));
    assert!(matches!(h.event().await, HostEvent::Hooks(_)));
    assert_eq!(
        h.event().await,
        HostEvent::Notes(vec![
            "ssh config line 3: Match blocks are not supported".into()
        ])
    );
    assert_eq!(h.event().await, HostEvent::Probe(ConnState::Up));
}
```

`crates/mai-core/tests/monitor.rs`（完整内容）：

```rust
//! `Monitor`: host events in, UI updates out.

use mai_core::host::{ConnState, HostEvent, HostState, Problem};
use mai_core::monitor::{Monitor, Update};
use mai_core::tracker::{AgentKey, TrackerConfig};
use mai_protocol::{
    AgentEvent, AgentState, EventSource, Metrics, PaneInfo, PaneRef, ProbeMsg, SessionInfo,
};

fn monitor() -> Monitor {
    Monitor::new(TrackerConfig::default())
}

fn event(session: &str, pane_id: u32, state: AgentState, ts_ms: u64) -> HostEvent {
    HostEvent::Msg(ProbeMsg::AgentEvent(AgentEvent {
        pane: PaneRef {
            session: session.into(),
            pane_id,
        },
        agent: "claude".into(),
        source: EventSource::Hook,
        state,
        message: None,
        ts_ms,
        spool_offset: Some(ts_ms),
    }))
}

fn pane(id: u32) -> PaneInfo {
    PaneInfo {
        id,
        tab_id: 0,
        tab_name: "t".into(),
        title: "x".into(),
        command: None,
        exited: false,
    }
}

fn key(session: &str, pane_id: u32) -> AgentKey {
    AgentKey {
        host_id: "h".into(),
        session: session.into(),
        pane_id,
    }
}

/// `(pane, state, acknowledged)` of every `Update::Agent`.
fn agents(updates: &[Update]) -> Vec<(u32, AgentState, bool)> {
    updates
        .iter()
        .filter_map(|u| match u {
            Update::Agent(r) => Some((r.key.pane_id, r.state, r.acknowledged)),
            _ => None,
        })
        .collect()
}

fn alerts(updates: &[Update]) -> Vec<AgentState> {
    updates
        .iter()
        .filter_map(|u| match u {
            Update::Alert(a) => Some(a.state),
            _ => None,
        })
        .collect()
}

#[test]
fn connection_state_becomes_host_state() {
    let mut m = monitor();
    let up = m.apply("h", HostEvent::Probe(ConnState::Up), 0);
    assert_eq!(
        up,
        vec![Update::Host {
            id: "h".into(),
            state: HostState::Online,
            probe: ConnState::Up,
            term: None,
        }]
    );
    let auth = ConnState::NeedsUser(Problem::Auth("denied".into()));
    let out = m.apply("h", HostEvent::Probe(auth.clone()), 0);
    assert!(matches!(
        &out[0],
        Update::Host {
            state: HostState::AuthRequired,
            ..
        }
    ));
    assert_eq!(m.host("h").unwrap().probe, Some(auth));
}

#[test]
fn terminal_connection_joins_the_host_state() {
    let mut m = monitor();
    m.apply("h", HostEvent::Probe(ConnState::Up), 0);
    let down = ConnState::Retrying {
        retry_in: std::time::Duration::from_secs(1),
        reason: "lost".into(),
    };
    let out = m.apply("h", HostEvent::Term(Some(down.clone())), 0);
    assert_eq!(
        out,
        vec![Update::Host {
            id: "h".into(),
            state: HostState::Degraded,
            probe: ConnState::Up,
            term: Some(down),
        }]
    );
    let out = m.apply("h", HostEvent::Term(Some(ConnState::Up)), 0);
    assert!(matches!(
        &out[0],
        Update::Host {
            state: HostState::Online,
            ..
        }
    ));
    let out = m.apply("h", HostEvent::Term(None), 0);
    assert!(matches!(
        &out[0],
        Update::Host {
            state: HostState::Online,
            term: None,
            ..
        }
    ));
    assert_eq!(m.host("h").unwrap().term, None);
}

#[test]
fn agent_changes_and_alerts_are_reported_once() {
    let mut m = monitor();
    let out = m.apply("h", event("w", 1, AgentState::Working, 10), 10);
    assert_eq!(agents(&out), vec![(1, AgentState::Working, true)]);
    assert!(alerts(&out).is_empty());

    let out = m.apply("h", event("w", 1, AgentState::NeedsInput, 20), 20);
    assert_eq!(agents(&out), vec![(1, AgentState::NeedsInput, false)]);
    assert_eq!(alerts(&out), vec![AgentState::NeedsInput]);

    let out = m.apply("h", event("w", 1, AgentState::NeedsInput, 30), 30);
    assert!(out.is_empty(), "{out:?}");
}

#[test]
fn acknowledge_reports_the_change_once() {
    let mut m = monitor();
    m.apply("h", event("w", 1, AgentState::Done, 10), 10);
    let u = m.acknowledge(&key("w", 1)).unwrap();
    assert_eq!(agents(&[u]), vec![(1, AgentState::Done, true)]);
    assert!(m.acknowledge(&key("w", 1)).is_none());
    assert!(m.acknowledge(&key("w", 99)).is_none());
}

#[test]
fn vanished_or_exited_pane_ends_its_agent() {
    let mut m = monitor();
    m.apply("h", event("w", 1, AgentState::Working, 10), 10);
    m.apply("h", event("w", 2, AgentState::Working, 10), 10);
    m.apply("h", event("x", 3, AgentState::Working, 10), 10);
    let mut exited = pane(2);
    exited.exited = true;
    let out = m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Panes {
            session: "w".into(),
            panes: vec![exited],
        }),
        50,
    );
    let mut gone = agents(&out);
    gone.sort_by_key(|g| g.0);
    assert_eq!(
        gone,
        vec![(1, AgentState::Exited, true), (2, AgentState::Exited, true)]
    );
    assert!(matches!(&out[0], Update::Panes { session, .. } if session == "w"));
    assert_eq!(
        m.tracker().get(&key("x", 3)).unwrap().state,
        AgentState::Working
    );

    let again = m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Panes {
            session: "w".into(),
            panes: vec![],
        }),
        60,
    );
    assert!(
        agents(&again).is_empty(),
        "already exited agents stay quiet"
    );
}

#[test]
fn ended_session_ends_its_agents_and_panes() {
    let mut m = monitor();
    m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Panes {
            session: "x".into(),
            panes: vec![pane(3)],
        }),
        0,
    );
    m.apply("h", event("x", 3, AgentState::Done, 10), 10);
    m.apply("h", event("w", 1, AgentState::Working, 10), 10);
    let out = m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Sessions {
            sessions: vec![
                SessionInfo {
                    name: "w".into(),
                    exited: false,
                },
                SessionInfo {
                    name: "x".into(),
                    exited: true,
                },
            ],
        }),
        20,
    );
    assert_eq!(agents(&out), vec![(3, AgentState::Exited, true)]);
    assert!(m.host("h").unwrap().panes.is_empty());
    assert!(
        m.tracker().pending().is_empty(),
        "exited agent no longer alerts"
    );
}

#[test]
fn agents_of_other_hosts_are_untouched() {
    let mut m = monitor();
    m.apply("other", event("w", 1, AgentState::Working, 10), 10);
    let out = m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Sessions { sessions: vec![] }),
        20,
    );
    assert!(agents(&out).is_empty());
}

#[test]
fn metrics_errors_and_heartbeats() {
    let mut m = monitor();
    let metrics = Metrics {
        cpu_pct: 12.5,
        mem_used: 1,
        mem_total: 2,
        disk_read_bps: 0,
        disk_write_bps: 0,
        net_rx_bps: 0,
        net_tx_bps: 0,
        load1: None,
        ts_ms: 5,
    };
    let out = m.apply("h", HostEvent::Msg(ProbeMsg::Metrics(metrics.clone())), 5);
    assert_eq!(
        out,
        vec![Update::Metrics {
            id: "h".into(),
            metrics: metrics.clone()
        }]
    );
    assert_eq!(m.host("h").unwrap().metrics, Some(metrics));
    let out = m.apply(
        "h",
        HostEvent::Msg(ProbeMsg::Error {
            code: "zellij".into(),
            message: "boom".into(),
        }),
        6,
    );
    assert!(matches!(&out[0], Update::ProbeError { code, .. } if code == "zellij"));
    assert!(
        m.apply("h", HostEvent::Msg(ProbeMsg::Heartbeat { ts_ms: 7 }), 7)
            .is_empty()
    );
}

#[test]
fn notes_are_passed_through() {
    let mut m = monitor();
    let out = m.apply("h", HostEvent::Notes(vec!["n".into()]), 0);
    assert_eq!(
        out,
        vec![Update::Notes {
            id: "h".into(),
            notes: vec!["n".into()]
        }]
    );
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test hosts --test monitor`
Expected: 编译失败（`Opened.notes`、`HostEvent::Notes`、`Update::Notes` 不存在）。

- [ ] **Step 3: `crates/mai-core/src/host.rs`（最终完整内容）**

```rust
//! One task per monitored host keeps its probe connection alive: connect,
//! deploy, start `serve`, relay messages, and reconnect with backoff.
//! Because only this task connects and deploys to its host, those steps
//! never run concurrently for one host.

use std::any::Any;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mai_protocol::{AppMsg, ProbeMsg};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Instant;

use crate::deploy::HookResult;
use crate::link::{HelloInfo, LinkError, ProbeIo, ProbeLink};
use crate::stderr::StderrTail;
use crate::term::TermTransport;

/// How the app reaches a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKind {
    /// This machine: the probe runs as a child process.
    Local,
    /// An `ssh` target: a `~/.ssh/config` alias or `[user@]host[:port]`.
    Ssh { target: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostConfig {
    /// Stable id, unique among hosts (used in `AgentKey::host_id`).
    pub id: String,
    pub kind: HostKind,
    /// zellij binary on the host when it is not found automatically.
    pub zellij: Option<String>,
}

/// Why a connection waits for the user instead of retrying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    /// No authentication method succeeded.
    Auth(String),
    /// The user declined the host key.
    HostKeyRejected,
    /// The host key differs from a recorded one.
    HostKeyChanged { file: PathBuf, line: usize },
    /// A known_hosts file marks the host key `@revoked`.
    HostKeyRevoked { file: PathBuf, line: usize },
    /// The probe could not be put on the host.
    Deploy(String),
    /// The host's configuration cannot be used (e.g. ssh config error).
    Config(String),
    /// The deployed probe speaks another protocol version.
    Protocol { probe: u32, app: u32 },
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auth(m) => write!(f, "authentication failed: {m}"),
            Self::HostKeyRejected => f.write_str("host key not accepted"),
            Self::HostKeyChanged { file, line } => write!(
                f,
                "HOST KEY CHANGED (recorded in {} line {line})",
                file.display()
            ),
            Self::HostKeyRevoked { file, line } => write!(
                f,
                "host key REVOKED (marked in {} line {line})",
                file.display()
            ),
            Self::Deploy(m) => write!(f, "probe deployment failed: {m}"),
            Self::Config(m) => write!(f, "configuration error: {m}"),
            Self::Protocol { probe, app } => {
                write!(
                    f,
                    "probe protocol {probe} does not match app protocol {app}"
                )
            }
        }
    }
}

/// Why `Connector::open` failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// Transient (network, timeout): retried with backoff.
    Retry(String),
    /// Needs the user: not retried until `HostCommand::Retry`.
    NeedsUser(Problem),
}

/// A started probe.
pub struct Opened {
    pub io: ProbeIo,
    /// Result of `install-hooks`. A failure only degrades detection to
    /// screen scraping, so it does not fail the connection.
    pub hooks: Result<Vec<HookResult>, String>,
    /// Kept alive while the probe runs (SSH session, child process).
    pub keep: Box<dyn Any + Send>,
    /// The probe's stderr, to explain why it stopped.
    pub stderr: Arc<StderrTail>,
    /// Things the user should know that did not stop the connection
    /// (ignored ssh config blocks, keychain errors).
    pub notes: Vec<String>,
}

/// `reason`, followed by the end of the probe's stderr if it wrote any.
async fn with_stderr(reason: String, stderr: &StderrTail) -> String {
    match stderr.summary().await {
        Some(tail) => format!("{reason} (probe stderr: {tail})"),
        None => reason,
    }
}

/// Reaches hosts: the probe connection (`open`: connect, deploy, start
/// `serve`) and the terminal connection (`open_terminals`).
pub trait Connector: Send + Sync + 'static {
    type Terminals: TermTransport;

    fn open(&self, host: &HostConfig) -> impl Future<Output = Result<Opened, OpenError>> + Send;

    fn open_terminals(
        &self,
        host: &HostConfig,
    ) -> impl Future<Output = Result<Self::Terminals, OpenError>> + Send;
}

/// State of one connection of a host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Connecting,
    Up,
    /// Down; the next attempt starts after `retry_in`.
    Retrying {
        retry_in: Duration,
        reason: String,
    },
    /// Down until the user acts (`HostCommand::Retry`).
    NeedsUser(Problem),
}

/// Overall host state shown in the UI (design 3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostState {
    Connecting,
    Online,
    /// Only one of the two connections is up.
    Degraded,
    Offline,
    AuthRequired,
}

fn needs_auth(c: &ConnState) -> bool {
    matches!(
        c,
        ConnState::NeedsUser(
            Problem::Auth(_)
                | Problem::HostKeyRejected
                | Problem::HostKeyChanged { .. }
                | Problem::HostKeyRevoked { .. }
        )
    )
}

/// Host state from its probe connection and, once the terminal
/// connection exists, the terminal connection (`None` until then).
pub fn host_state(probe: &ConnState, term: Option<&ConnState>) -> HostState {
    let conns: Vec<&ConnState> = std::iter::once(probe).chain(term).collect();
    let up = conns.iter().filter(|c| ***c == ConnState::Up).count();
    if up == conns.len() {
        HostState::Online
    } else if up > 0 {
        HostState::Degraded
    } else if conns.iter().any(|c| needs_auth(c)) {
        HostState::AuthRequired
    } else if conns.iter().any(|c| **c == ConnState::Connecting) {
        HostState::Connecting
    } else {
        HostState::Offline
    }
}

/// Reconnect delays: 1 s doubling to 60 s (design 3.4).
#[derive(Debug, Clone)]
pub struct Backoff {
    next: Duration,
}

impl Backoff {
    pub const FIRST: Duration = Duration::from_secs(1);
    pub const MAX: Duration = Duration::from_secs(60);

    /// The delay to wait now; the following one doubles.
    pub fn next_delay(&mut self) -> Duration {
        let d = self.next;
        self.next = (d * 2).min(Self::MAX);
        d
    }

    pub fn reset(&mut self) {
        self.next = Self::FIRST;
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self { next: Self::FIRST }
    }
}

/// A probe that stayed up this long resets the backoff when it drops,
/// so a probe that dies right after starting keeps backing off.
pub const STABLE_AFTER: Duration = Duration::from_secs(30);

/// Sent by a host task to the monitor.
#[derive(Debug, Clone, PartialEq)]
pub enum HostEvent {
    Probe(ConnState),
    Hello(HelloInfo),
    Hooks(Result<Vec<HookResult>, String>),
    Msg(ProbeMsg),
    /// A command could not be delivered (probe not connected).
    Dropped(AppMsg),
    /// Terminal connection state; `None` while no terminal is open.
    Term(Option<ConnState>),
    /// Notes from the latest connect (see `Opened::notes`).
    Notes(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum HostCommand {
    /// Connect now: skips a backoff wait or leaves `NeedsUser`.
    Retry,
    Send(AppMsg),
    Stop,
}

type Events = UnboundedSender<(String, HostEvent)>;

struct Host<C> {
    cfg: HostConfig,
    connector: Arc<C>,
    events: Events,
    cmds: UnboundedReceiver<HostCommand>,
}

/// What a command received while waiting means for the wait.
enum Wake {
    Retry,
    Stop,
}

impl<C: Connector> Host<C> {
    fn emit(&self, ev: HostEvent) {
        let _ = self.events.send((self.cfg.id.clone(), ev));
    }

    /// Handle a command that arrived while no probe is running.
    fn idle_command(&self, cmd: Option<HostCommand>) -> Option<Wake> {
        match cmd {
            None | Some(HostCommand::Stop) => Some(Wake::Stop),
            Some(HostCommand::Retry) => Some(Wake::Retry),
            Some(HostCommand::Send(m)) => {
                self.emit(HostEvent::Dropped(m));
                None
            }
        }
    }

    /// Run `fut` while answering commands; `None` if told to stop.
    /// `Retry` is ignored: an attempt is already under way.
    async fn busy<F: Future>(&mut self, fut: F) -> Option<F::Output> {
        tokio::pin!(fut);
        loop {
            tokio::select! {
                out = &mut fut => return Some(out),
                cmd = self.cmds.recv() => {
                    if let Some(Wake::Stop) = self.idle_command(cmd) {
                        return None;
                    }
                }
            }
        }
    }

    /// Wait `delay` (forever if `None`) or until `Retry`; false on stop.
    async fn wait(&mut self, delay: Option<Duration>) -> bool {
        // Without a delay the sleep branch is disabled; its length is moot.
        let sleep = tokio::time::sleep(delay.unwrap_or(Duration::ZERO));
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep, if delay.is_some() => return true,
                cmd = self.cmds.recv() => match self.idle_command(cmd) {
                    Some(Wake::Retry) => return true,
                    Some(Wake::Stop) => return false,
                    None => {}
                },
            }
        }
    }

    /// Relay messages until the link fails (`Some(reason)`) or the task
    /// is told to stop (`None`). Hook events are acknowledged once they
    /// have been handed to the monitor.
    async fn serve(&mut self, link: &mut ProbeLink) -> Option<LinkError> {
        loop {
            tokio::select! {
                msg = link.recv() => {
                    let msg = match msg {
                        Ok(m) => m,
                        Err(e) => return Some(e),
                    };
                    let ack = match &msg {
                        ProbeMsg::AgentEvent(ev) => ev.spool_offset,
                        _ => None,
                    };
                    self.emit(HostEvent::Msg(msg));
                    if let Some(spool_offset) = ack
                        && let Err(e) = link.send(&AppMsg::Ack { spool_offset }).await
                    {
                        return Some(e);
                    }
                }
                cmd = self.cmds.recv() => match cmd {
                    None | Some(HostCommand::Stop) => return None,
                    Some(HostCommand::Retry) => {}
                    Some(HostCommand::Send(m)) => {
                        if let Err(e) = link.send(&m).await {
                            self.emit(HostEvent::Dropped(m));
                            return Some(e);
                        }
                    }
                },
            }
        }
    }

    async fn run(mut self) {
        let mut backoff = Backoff::default();
        loop {
            self.emit(HostEvent::Probe(ConnState::Connecting));
            let connector = self.connector.clone();
            let cfg = self.cfg.clone();
            let Some(opened) = self.busy(connector.open(&cfg)).await else {
                return;
            };
            let reason = match opened {
                Err(OpenError::NeedsUser(p)) => {
                    self.emit(HostEvent::Probe(ConnState::NeedsUser(p)));
                    if !self.wait(None).await {
                        return;
                    }
                    backoff.reset();
                    continue;
                }
                Err(OpenError::Retry(reason)) => reason,
                Ok(opened) => {
                    let mut link = ProbeLink::new(opened.io);
                    let Some(hello) = self.busy(link.hello()).await else {
                        return;
                    };
                    match hello {
                        Err(LinkError::Protocol { probe, app }) => {
                            let p = Problem::Protocol { probe, app };
                            self.emit(HostEvent::Probe(ConnState::NeedsUser(p)));
                            if !self.wait(None).await {
                                return;
                            }
                            continue;
                        }
                        Err(e) => with_stderr(e.to_string(), &opened.stderr).await,
                        Ok(hello) => {
                            self.emit(HostEvent::Hello(hello));
                            self.emit(HostEvent::Hooks(opened.hooks));
                            if !opened.notes.is_empty() {
                                self.emit(HostEvent::Notes(opened.notes));
                            }
                            self.emit(HostEvent::Probe(ConnState::Up));
                            let up_since = Instant::now();
                            let Some(e) = self.serve(&mut link).await else {
                                return;
                            };
                            if up_since.elapsed() >= STABLE_AFTER {
                                backoff.reset();
                            }
                            with_stderr(e.to_string(), &opened.stderr).await
                        }
                    }
                }
            };
            let retry_in = backoff.next_delay();
            self.emit(HostEvent::Probe(ConnState::Retrying { retry_in, reason }));
            if !self.wait(Some(retry_in)).await {
                return;
            }
        }
    }
}

/// Run the probe connection of `cfg` until `HostCommand::Stop` or until
/// the command channel closes.
pub async fn run_host<C: Connector>(
    cfg: HostConfig,
    connector: Arc<C>,
    events: Events,
    cmds: UnboundedReceiver<HostCommand>,
) {
    Host {
        cfg,
        connector,
        events,
        cmds,
    }
    .run()
    .await
}
```

- [ ] **Step 4: `crates/mai-core/src/monitor.rs`（最终完整内容）**

```rust
//! Aggregates the events of every host into one view: host states, zellij
//! sessions and panes, agent states (via `Tracker`) and alerts. Pure and
//! synchronous; the manager task feeds it and forwards its `Update`s.

use std::collections::{BTreeMap, HashMap};

use mai_protocol::{AgentState, AppMsg, Metrics, PaneInfo, ProbeMsg, SessionInfo};

use crate::deploy::HookResult;
use crate::host::{ConnState, HostEvent, HostState, host_state};
use crate::link::HelloInfo;
use crate::tracker::{AgentKey, AgentRecord, Alert, Tracker, TrackerConfig};

/// A change the UI should show.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// A connection of the host changed. `term` is `None` while no
    /// terminal is open.
    Host {
        id: String,
        state: HostState,
        probe: ConnState,
        term: Option<ConnState>,
    },
    Hello {
        id: String,
        info: HelloInfo,
    },
    Hooks {
        id: String,
        result: Result<Vec<HookResult>, String>,
    },
    Sessions {
        id: String,
        sessions: Vec<SessionInfo>,
    },
    Panes {
        id: String,
        session: String,
        panes: Vec<PaneInfo>,
    },
    /// An agent's state or acknowledgement changed.
    Agent(AgentRecord),
    /// The user should be notified.
    Alert(Alert),
    Metrics {
        id: String,
        metrics: Metrics,
    },
    ProbeError {
        id: String,
        code: String,
        message: String,
    },
    /// Things the user should know about a host that did not stop it.
    Notes {
        id: String,
        notes: Vec<String>,
    },
    /// A command could not be delivered because the probe was down.
    Dropped {
        id: String,
        msg: AppMsg,
    },
}

/// What is known about one host.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HostView {
    pub probe: Option<ConnState>,
    /// Terminal connection; `None` while no terminal is open.
    pub term: Option<ConnState>,
    pub hello: Option<HelloInfo>,
    pub sessions: Vec<SessionInfo>,
    pub panes: BTreeMap<String, Vec<PaneInfo>>,
    pub metrics: Option<Metrics>,
}

pub struct Monitor {
    tracker: Tracker,
    hosts: HashMap<String, HostView>,
}

impl Monitor {
    pub fn new(cfg: TrackerConfig) -> Self {
        Self {
            tracker: Tracker::new(cfg),
            hosts: HashMap::new(),
        }
    }

    pub fn tracker(&self) -> &Tracker {
        &self.tracker
    }

    pub fn host(&self, id: &str) -> Option<&HostView> {
        self.hosts.get(id)
    }

    /// Mark an agent's alert as seen (the user opened its pane).
    pub fn acknowledge(&mut self, key: &AgentKey) -> Option<Update> {
        let was = self.tracker.get(key)?.acknowledged;
        self.tracker.acknowledge(key);
        let rec = self.tracker.get(key)?;
        (!was).then(|| Update::Agent(rec.clone()))
    }

    /// Forget a host that was removed from the configuration.
    pub fn remove_host(&mut self, id: &str) {
        self.hosts.remove(id);
    }

    /// Host state from both connections; a probe connection not reported
    /// yet counts as connecting.
    fn host_update(id: String, view: &HostView) -> Update {
        let probe = view.probe.clone().unwrap_or(ConnState::Connecting);
        Update::Host {
            state: host_state(&probe, view.term.as_ref()),
            id,
            probe,
            term: view.term.clone(),
        }
    }

    pub fn apply(&mut self, id: &str, ev: HostEvent, now_ms: u64) -> Vec<Update> {
        let id = id.to_owned();
        let view = self.hosts.entry(id.clone()).or_default();
        match ev {
            HostEvent::Probe(probe) => {
                view.probe = Some(probe);
                vec![Self::host_update(id, view)]
            }
            HostEvent::Term(term) => {
                view.term = term;
                vec![Self::host_update(id, view)]
            }
            HostEvent::Hello(info) => {
                view.hello = Some(info.clone());
                vec![Update::Hello { id, info }]
            }
            HostEvent::Hooks(result) => vec![Update::Hooks { id, result }],
            HostEvent::Notes(notes) => vec![Update::Notes { id, notes }],
            HostEvent::Dropped(msg) => vec![Update::Dropped { id, msg }],
            HostEvent::Msg(msg) => self.apply_msg(id, msg, now_ms),
        }
    }

    fn apply_msg(&mut self, id: String, msg: ProbeMsg, now_ms: u64) -> Vec<Update> {
        let view = self.hosts.entry(id.clone()).or_default();
        match msg {
            ProbeMsg::Sessions { sessions } => {
                view.panes
                    .retain(|name, _| sessions.iter().any(|s| &s.name == name && !s.exited));
                view.sessions = sessions.clone();
                let live =
                    |session: &str, _: u32| sessions.iter().any(|s| s.name == session && !s.exited);
                let mut out = vec![Update::Sessions {
                    id: id.clone(),
                    sessions: sessions.clone(),
                }];
                out.extend(self.agents_gone(&id, None, live, now_ms));
                out
            }
            ProbeMsg::Panes { session, panes } => {
                view.panes.insert(session.clone(), panes.clone());
                let live = |_: &str, pane: u32| panes.iter().any(|p| p.id == pane && !p.exited);
                let mut out = vec![Update::Panes {
                    id: id.clone(),
                    session: session.clone(),
                    panes: panes.clone(),
                }];
                out.extend(self.agents_gone(&id, Some(&session), live, now_ms));
                out
            }
            ProbeMsg::AgentEvent(ev) => {
                let key = AgentKey::from_event(&id, &ev);
                let summary = |r: &AgentRecord| (r.state, r.acknowledged);
                let before = self.tracker.get(&key).map(summary);
                let alert = self.tracker.apply(&id, &ev);
                let mut out = Vec::new();
                // A new agent, a new state or a new unacknowledged alert.
                if let Some(rec) = self.tracker.get(&key)
                    && before != Some(summary(rec))
                {
                    out.push(Update::Agent(rec.clone()));
                }
                out.extend(alert.map(Update::Alert));
                out
            }
            ProbeMsg::Metrics(metrics) => {
                view.metrics = Some(metrics.clone());
                vec![Update::Metrics { id, metrics }]
            }
            ProbeMsg::Error { code, message } => vec![Update::ProbeError { id, code, message }],
            ProbeMsg::Hello { .. } | ProbeMsg::Heartbeat { .. } => Vec::new(),
        }
    }

    /// Mark agents of host `id` (in `session`, if given) as exited when
    /// `live(session, pane)` says their pane is gone.
    fn agents_gone(
        &mut self,
        id: &str,
        session: Option<&str>,
        live: impl Fn(&str, u32) -> bool,
        now_ms: u64,
    ) -> Vec<Update> {
        let gone: Vec<AgentKey> = self
            .tracker
            .records()
            .filter(|r| r.key.host_id == id)
            .filter(|r| session.is_none_or(|s| r.key.session == s))
            .filter(|r| r.state != AgentState::Exited)
            .filter(|r| !live(&r.key.session, r.key.pane_id))
            .map(|r| r.key.clone())
            .collect();
        gone.into_iter()
            .filter_map(|key| {
                self.tracker.pane_gone(&key, now_ms);
                self.tracker.get(&key).cloned().map(Update::Agent)
            })
            .collect()
    }
}
```

- [ ] **Step 5: `crates/mai-core/src/connect.rs`（最终完整内容）**

```rust
//! The real `Connector`: SSH hosts get the probe deployed over SSH and
//! `serve` started on an exec channel; the local host gets the probe
//! copied into the home directory and `serve` started as a child process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};

use tokio::io::BufReader;

use crate::deploy::{
    DeployError, DeployOptions, HookResult, ProbeStore, Remote, Shell, deploy, detect,
    hooks_result, normalize_arch, serve_argv, sha256_hex, stop_argv,
};
use crate::host::{Connector, HostConfig, HostKind, OpenError, Opened, Problem};
use crate::link::ProbeIo;
use crate::ssh::auth::{Prompter, SecretStore};
use crate::ssh::client::{ConnectOptions, ExecOutput, SshError, connect};
use crate::ssh::config::{HostSpec, config_warnings, parse_config, resolve};
use crate::stderr::{StderrTail, collect as collect_stderr};
use crate::swap::{LocalFiles, swap_in};
use crate::terminals::SystemTerminals;

/// Locale requested for `serve` so zellij output is UTF-8.
const LOCALE: &str = "en_US.UTF-8";

/// Map an SSH failure to retry-or-ask-the-user.
pub fn ssh_open_error(e: SshError) -> OpenError {
    match e {
        SshError::Connect(m) | SshError::Channel(m) => OpenError::Retry(m),
        SshError::Auth(m) => OpenError::NeedsUser(Problem::Auth(m)),
        SshError::HostKeyRejected { .. } => OpenError::NeedsUser(Problem::HostKeyRejected),
        SshError::HostKeyChanged { file, line, .. } => {
            OpenError::NeedsUser(Problem::HostKeyChanged { file, line })
        }
        SshError::HostKeyRevoked { file, line, .. } => {
            OpenError::NeedsUser(Problem::HostKeyRevoked { file, line })
        }
    }
}

/// Map a deployment failure to retry-or-ask-the-user.
pub fn deploy_open_error(e: DeployError) -> OpenError {
    match e {
        DeployError::Ssh(e) => ssh_open_error(e),
        other => OpenError::NeedsUser(Problem::Deploy(other.to_string())),
    }
}

fn hooks_outcome(out: &ExecOutput) -> Result<Vec<HookResult>, String> {
    hooks_result(out).map_err(|e| e.to_string())
}

/// Remote-style description of this machine, for choosing the probe
/// binary and building command lines.
pub fn local_remote(home: &Path) -> Option<Remote> {
    use crate::deploy::Os;
    let os = match std::env::consts::OS {
        "linux" => Os::Linux,
        "macos" => Os::MacOs,
        "windows" => Os::Windows,
        _ => return None,
    };
    Some(Remote {
        os,
        arch: normalize_arch(std::env::consts::ARCH)?,
        shell: if os == Os::Windows {
            Shell::Cmd
        } else {
            Shell::Posix
        },
        home: home.to_string_lossy().into_owned(),
    })
}

/// Put `bytes` at `exe` unless it already has them. `stop` runs first
/// when an existing, different binary is replaced; the old binary is then
/// moved aside rather than deleted (a running probe locks its file on
/// Windows), see `swap::swap_in`. Returns whether the file was written.
pub async fn place_binary(
    exe: &Path,
    bytes: &[u8],
    stop: impl Future<Output = ()>,
) -> std::io::Result<bool> {
    let existing = tokio::fs::read(exe).await.ok();
    if existing.as_deref().map(sha256_hex) == Some(sha256_hex(bytes)) {
        return Ok(false);
    }
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else {
        return Err(std::io::Error::other("probe path has no directory"));
    };
    tokio::fs::create_dir_all(dir).await?;
    let mut tmp = exe.as_os_str().to_owned();
    tmp.push(".upload");
    let tmp = PathBuf::from(tmp);
    tokio::fs::write(&tmp, bytes).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).await?;
    }
    if existing.is_some() {
        stop.await;
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    swap_in(
        &LocalFiles,
        &dir.to_string_lossy(),
        &name.to_string_lossy(),
        stamp,
    )
    .await?;
    Ok(true)
}

#[cfg(windows)]
fn no_window(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    cmd.creation_flags(0x0800_0000)
}

#[cfg(not(windows))]
fn no_window(cmd: &mut tokio::process::Command) -> &mut tokio::process::Command {
    cmd
}

/// Run `exe` with `argv` and collect its output.
async fn run_local(exe: &Path, argv: &[String]) -> std::io::Result<ExecOutput> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(argv).stdin(Stdio::null());
    let out = no_window(&mut cmd).output().await?;
    Ok(ExecOutput {
        status: out.status.code().and_then(|c| u32::try_from(c).ok()),
        stdout: out.stdout,
        stderr: out.stderr,
    })
}

/// Everything needed to reach hosts and start their probes.
pub struct SystemConnector<P, S> {
    pub opts: ConnectOptions,
    pub prompter: Arc<P>,
    pub secrets: Arc<S>,
    pub probes: ProbeStore,
    /// `~/.ssh/config`; read on every connect so edits take effect.
    pub ssh_config: Option<PathBuf>,
    /// Local home: default keys, and where the local probe is installed.
    pub home: PathBuf,
    /// User for targets that name none.
    pub default_user: String,
    /// This app installation's id (see `mai-probe serve --client`).
    pub client: String,
    /// Deploy dir under the home directory (`.mai`).
    pub dir: String,
    /// Install agent hooks after deploying.
    pub install_hooks: bool,
    gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl<P: Prompter, S: SecretStore> SystemConnector<P, S> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        opts: ConnectOptions,
        prompter: Arc<P>,
        secrets: Arc<S>,
        probes: ProbeStore,
        ssh_config: Option<PathBuf>,
        home: PathBuf,
        default_user: String,
        client: String,
    ) -> Self {
        Self {
            opts,
            prompter,
            secrets,
            probes,
            ssh_config,
            home,
            default_user,
            client,
            dir: ".mai".to_owned(),
            install_hooks: true,
            gates: Mutex::new(HashMap::new()),
        }
    }

    /// Lock held while connecting to and deploying on host `id`, so
    /// host-key prompts and uploads for one host never overlap.
    pub fn gate(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(|p| p.into_inner());
        gates.entry(id.to_owned()).or_default().clone()
    }

    /// The resolved target and the config warnings (ignored `Match` blocks).
    fn spec(&self, target: &str) -> Result<(HostSpec, Vec<String>), OpenError> {
        let text = match &self.ssh_config {
            None => String::new(),
            Some(p) => match std::fs::read_to_string(p) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => {
                    return Err(OpenError::NeedsUser(Problem::Config(format!(
                        "{}: {e}",
                        p.display()
                    ))));
                }
            },
        };
        let config = parse_config(&text)
            .map_err(|e| OpenError::NeedsUser(Problem::Config(e.to_string())))?;
        let spec = resolve(&config, target, &self.default_user, &self.home)
            .map_err(|e| OpenError::NeedsUser(Problem::Config(e.to_string())))?;
        Ok((spec, config_warnings(&text)))
    }

    async fn open_ssh(&self, host: &HostConfig, target: &str) -> Result<Opened, OpenError> {
        let (spec, mut notes) = self.spec(target)?;
        let gate = self.gate(&host.id);
        let guard = gate.lock().await;
        let session = connect(&spec, &self.opts, self.prompter.clone(), &*self.secrets)
            .await
            .map_err(ssh_open_error)?;
        let opts = DeployOptions {
            dir: self.dir.clone(),
            install_hooks: false,
            client: self.client.clone(),
        };
        let report = deploy(&session, &self.probes, &opts)
            .await
            .map_err(deploy_open_error)?;
        notes.extend(session.notes().iter().cloned());
        let remote = &report.remote;
        let hooks = if self.install_hooks {
            let cmd = remote.invoke(&report.probe_path, "install-hooks");
            let out = session.exec(&cmd).await.map_err(ssh_open_error)?;
            hooks_outcome(&out)
        } else {
            Ok(Vec::new())
        };
        drop(guard);
        let args = remote.serve_args(&self.client, host.zellij.as_deref());
        let env = [("LANG", LOCALE), ("LC_CTYPE", LOCALE)];
        let stderr = Arc::new(StderrTail::default());
        let (r, w) = session
            .open_exec_split(
                &remote.invoke(&report.probe_path, &args),
                &env,
                stderr.clone(),
            )
            .await
            .map_err(ssh_open_error)?;
        Ok(Opened {
            io: ProbeIo {
                reader: Box::new(BufReader::new(r)),
                writer: w,
            },
            hooks,
            stderr,
            notes,
            keep: Box::new(session),
        })
    }

    async fn open_local(&self, host: &HostConfig) -> Result<Opened, OpenError> {
        let deploy_err = |m: String| OpenError::NeedsUser(Problem::Deploy(m));
        let remote = local_remote(&self.home)
            .ok_or_else(|| deploy_err("unsupported local platform".into()))?;
        let target = remote
            .target()
            .ok_or_else(|| deploy_err("unsupported local platform".into()))?;
        let bin = self.probes.binary(target);
        let bytes =
            std::fs::read(&bin).map_err(|e| deploy_err(format!("{}: {e}", bin.display())))?;
        let exe = PathBuf::from(remote.probe_path(&self.dir));
        let gate = self.gate(&host.id);
        let guard = gate.lock().await;
        let stop = async {
            let _ = run_local(&exe, &stop_argv(&self.client)).await;
        };
        place_binary(&exe, &bytes, stop)
            .await
            .map_err(|e| deploy_err(format!("{}: {e}", exe.display())))?;
        let hooks = if self.install_hooks {
            match run_local(&exe, &["install-hooks".to_owned()]).await {
                Ok(out) => hooks_outcome(&out),
                Err(e) => Err(e.to_string()),
            }
        } else {
            Ok(Vec::new())
        };
        drop(guard);
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.args(serve_argv(&self.client, host.zellij.as_deref()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = no_window(&mut cmd)
            .spawn()
            .map_err(|e| OpenError::Retry(format!("start {}: {e}", exe.display())))?;
        let (Some(out), Some(input), Some(err)) =
            (child.stdout.take(), child.stdin.take(), child.stderr.take())
        else {
            return Err(OpenError::Retry("probe pipes unavailable".into()));
        };
        let stderr = Arc::new(StderrTail::default());
        tokio::spawn(collect_stderr(err, stderr.clone()));
        Ok(Opened {
            io: ProbeIo {
                reader: Box::new(BufReader::new(out)),
                writer: Box::new(input),
            },
            hooks,
            keep: Box::new(child),
            stderr,
            notes: Vec::new(),
        })
    }
}

impl<P: Prompter, S: SecretStore> SystemConnector<P, S> {
    /// A second SSH session for terminals. Connecting holds the host's
    /// gate, so it never prompts for a host key at the same time as the
    /// probe connection.
    async fn open_ssh_terminals(
        &self,
        host: &HostConfig,
        target: &str,
    ) -> Result<SystemTerminals<P>, OpenError> {
        let (spec, _) = self.spec(target)?;
        let gate = self.gate(&host.id);
        let _guard = gate.lock().await;
        let session = connect(&spec, &self.opts, self.prompter.clone(), &*self.secrets)
            .await
            .map_err(ssh_open_error)?;
        let remote = detect(&session).await.map_err(deploy_open_error)?;
        Ok(SystemTerminals::Ssh { session, remote })
    }
}

impl<P: Prompter, S: SecretStore> Connector for SystemConnector<P, S> {
    type Terminals = SystemTerminals<P>;

    async fn open(&self, host: &HostConfig) -> Result<Opened, OpenError> {
        match &host.kind {
            HostKind::Local => self.open_local(host).await,
            HostKind::Ssh { target } => self.open_ssh(host, target).await,
        }
    }

    async fn open_terminals(&self, host: &HostConfig) -> Result<SystemTerminals<P>, OpenError> {
        match &host.kind {
            HostKind::Local => Ok(SystemTerminals::Local),
            HostKind::Ssh { target } => self.open_ssh_terminals(host, target).await,
        }
    }
}
```

- [ ] **Step 6: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: `hosts` 17 个、`monitor` 9 个测试通过，全部 243 个；clippy 无警告。
另检查源码只含 ASCII：

```bash
LC_ALL=C grep -rn '[^[:print:][:space:]]' crates/*/src || echo ascii-ok
```

Expected: `ascii-ok`。

- [ ] **Step 7: 提交**

```bash
git add crates/mai-core
git commit -m "feat(core): pass connect notes (ignored Match blocks, keychain failures) to the UI

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: 端到端验证与文档

**Files:**

- Modify: `README.md`、`docs/superpowers/plans/2026-09-24-02-followups.md`

- [ ] **Step 1: Windows 本机端到端（替换被锁住的探针）**

先用 `main` 最近一次 CI 的探针部署一个旧版本，再在它被另一个 client 的 `serve`
锁住时部署本分支构建的新版本（两者 SHA-256 不同即可）：

```bash
gh run list --repo phalanger/multi-ai --branch main --limit 1
gh run download <main-run-id> --repo phalanger/multi-ai --name probes --dir probes-old
cargo build -p mai-probe --release --locked
mkdir -p probes/x86_64-pc-windows-msvc
cp target/release/mai-probe.exe probes/x86_64-pc-windows-msvc/
cargo run -p mai-core --example monitor -- probes-old 10 local
(sleep 40 | ~/.mai-e2e/bin/mai-probe.exe serve --client other > /dev/null 2>&1 &)
cargo run -p mai-core --example monitor -- probes 10 local
ls ~/.mai-e2e/bin
sha256sum ~/.mai-e2e/bin/mai-probe.exe probes/x86_64-pc-windows-msvc/mai-probe.exe
```

Expected：两次运行都依次出现 `Hello { id: "local", .. }`、
`Hooks { id: "local", result: Ok([]) }`、
`Host { id: "local", state: Online, probe: Up, .. }`；`ls` 为 `mai-probe.exe` 与
`mai-probe.exe.old`（旧文件仍被 `other` 锁住，下次替换时删除）；两个 SHA-256 相同。

清理：

```powershell
Get-Process mai-probe -ErrorAction SilentlyContinue | Where-Object { $_.Path -like '*\.mai-e2e\*' } | Stop-Process -Force
Remove-Item -Recurse -Force -LiteralPath "$env:USERPROFILE\.mai-e2e"
Get-Process mai-probe -ErrorAction SilentlyContinue
```

最后一条应无输出。删除本地 `probes-old/`（`probes/` 留给 Step 5）。

- [ ] **Step 2: 更新 `README.md`**

`## mai-core` 小节：

- `ssh::config::resolve` 一条改为：

```markdown
- `ssh::config::resolve` turns an alias or `user@host:port` into a `HostSpec`
  using `~/.ssh/config` (HostName, User, Port, IdentityFile, ProxyJump; a
  jump host's own ProxyJump is not followed). `Match` blocks are ignored and
  reported by `config_warnings`.
```

- `ssh::client::connect` 一条改为：

```markdown
- `ssh::client::connect` authenticates with key files (the public key is
  offered first; an encrypted key is decrypted only once the server accepts
  it), ssh-agent, remembered or prompted password, then keyboard-interactive.
  Host keys are checked against known_hosts (hashed names, globs, `@revoked`);
  unknown keys are confirmed through `Prompter`, changed or revoked keys are
  refused.
```

- `deploy::deploy` 一条改为：

```markdown
- `deploy::deploy` uploads the matching probe (skipped when SHA-256 matches),
  checks the uploaded file's SHA-256 on the host, swaps it in by moving the
  old binary aside (`swap::swap_in`, works while it runs on Windows) and runs
  `install-hooks`.
```

- `monitor::Monitor` 那句末尾的
  `(host state, sessions, panes, agents, alerts, metrics)` 改为
  `(host state, sessions, panes, agents, alerts, metrics, connect notes)`。

- [ ] **Step 3: 更新 `docs/superpowers/plans/2026-09-24-02-followups.md`**

- “03d 必须处理”小节标题改为“03e 必须处理”。
- 以下各条末尾加“**（已在 03d 处理）**”：B4、B5、B6、B8、B9、B11、B12、B14、B16、B19。
- B7 末尾加：“**（03d：keyboard-interactive 最多 10 轮；prompter 不应答时仍会
  一直等待，由应用的弹窗负责超时或取消）**”。
- B10 的“（03b 部分处理……）”之后加：“**（03d：`learn` 不再写重复行）**”。
- B13 末尾加：“**（03d：已加经跳板机的进程内集成测试；真实网络仍未验证）**”。
- B18 末尾加：“**（03d：上传后在主机上校验 SHA-256，旧文件改名挪开再替换；
  SFTP 不可用时回退 exec 写入仍未实现）**”。
- B1 末尾加：“**（03d 未能验证：本机 sshd 的公钥登录尚未配置；配置后与 B20
  一起验证，不写钥匙串）**”。
- B3 末尾加：“**（03d：钥匙串写入或删除失败会作为提示上报；测试不写真实钥匙串，
  `KeyringStore` 的首次真实使用留到 04 手工验证）**”。

Run: `npx -y markdownlint-cli2 README.md docs/superpowers/specs/*.md docs/superpowers/plans/*.md`
Expected: 0 issues。

- [ ] **Step 4: 提交、推送分支、等待 CI**

```bash
git add README.md docs
git commit -m "docs: 03d SSH and deploy hardening; follow-ups

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u origin feat/03d-ssh-deploy-hardening
gh run watch --repo phalanger/multi-ai --exit-status
```

Expected: CI 三平台测试与 5 个探针目标全部通过。

- [ ] **Step 5: Ubuntu 端到端（用 CI 产出的探针，旧版本替换为新版本）**

```bash
gh run download <branch-run-id> --repo phalanger/multi-ai --name probes --dir probes-new
ssh ubuntu@100.66.61.30 'ls -d ~/.mai-e2e 2>/dev/null || echo none'
cargo run -p mai-core --example monitor -- probes-old 120 ubuntu@100.66.61.30
cargo run -p mai-core --example monitor -- probes-new 120 ubuntu@100.66.61.30
ssh ubuntu@100.66.61.30 'ls ~/.mai-e2e/bin; sha256sum ~/.mai-e2e/bin/mai-probe'
sha256sum probes-new/x86_64-unknown-linux-musl/mai-probe
```

（`probes-old` 按 Step 1 重新下载 `main` 的产物；Ubuntu 链路慢，上传需要较长时间，
故运行 120 秒。）

Expected：第一条 ssh 输出 `none`；两次运行都出现 `Hello { .. os: "linux" .. }` 与
`Host { .. state: Online, probe: Up .. }`；`ls` 只有 `mai-probe`（旧文件未被锁住，
已删除）；两个 SHA-256 相同。

清理（monitor 示例不安装 hook，不创建 session）：

```bash
ssh ubuntu@100.66.61.30 'pgrep -fl mai-probe || echo none; rm -rf ~/.mai-e2e; ls -d ~/.mai 2>/dev/null || echo "no ~/.mai"'
```

Expected：`none`、`no ~/.mai`；本机删除 `probes/`、`probes-old/`、`probes-new/`。
Mac 端到端只在用户同意后按同样步骤进行（`cyt@100.96.237.7`，目标
`aarch64-apple-darwin`）。

## 已知限制（留给 03e / 04）

- 上传内容校验失败的分支只有代码审查覆盖（需要可控的 SFTP 服务器才能造出）。
- SFTP 不可用时没有 exec 写入回退（B18）。
- 两个应用同时连接同一未知主机仍会各自弹窗确认（B10）。
- keyboard-interactive 的 prompter 不应答时一直等待（B7，由应用弹窗处理）。
- 需要特定环境的人工验证：Windows sshd 服务会话与 cmd 引号（B1、B20）、
  ssh-agent 与真实 2FA（B2、B15）、`KeyringStore` 真实使用（B3）。
- 终端加固（B25-B33）在 03e。
