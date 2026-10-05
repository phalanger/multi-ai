# 03e 终端加固与 SSH 遗留项 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 处理终端加固项 B26-B32（SSH PTY 建立、终端重连、连接期间的命令、
主机重加入、zellij 路径、session 名、本机 locale）与 03d 留下的 SSH/部署项
B34-B40（上传超时、跳板主机提示、失败时的提示、测试不接触真实 agent、口令键迁移、
签名错误文本）；B36 按用户决定保持严格。

**Architecture:** `ssh::pty` 把“请求无应答”当作建立失败并关闭 channel；
`term` 的终端任务在连接与 attach 期间用 `busy()` 继续处理命令，按
`TermTransport::alive` 区分“连接断开”与“单个 channel 关闭”，并按
`STABLE_AFTER` 复位退避；`manager` 给每次 `add_host` 一个代数，由转发任务给
事件打上代数后交给监视任务过滤；`terminals` 在主机上查找 zellij、只在没有 UTF-8
locale 时补上 locale；`connect::with_notes` 把提示附加到失败原因上。

**Tech Stack:** Rust 1.92（edition 2024）、tokio、russh 0.63、russh-sftp 3、
既有的 mai-core / mai-protocol / mai-probe；不新增依赖。

**Spec:** `docs/superpowers/specs/2026-09-23-multi-ai-monitor-design.md`
（第 3.1、3.2、3.3、3.4、4.2、7、11 节，已按本计划更新）；遗留事项
`docs/superpowers/plans/2026-09-24-02-followups.md`。

## Global Constraints

- 源码（代码、字符串、注释）只含 ASCII；测试、示例与文档可用中文。
- 零警告：`cargo build` 无警告；
  `cargo clippy --workspace --all-targets --locked -- -D warnings` 通过。
- 本计划给出的每个文件内容都已在 Windows 上逐任务编译、测试（各任务结束时的
  测试总数见各任务；最终 271 个，Windows 上），在 CI 三平台通过、5 个探针目标构建
  成功；本机终端端到端已在 Windows 验证。**逐字写入**。
  用编辑器或文件写入工具写，或从本计划机械提取代码块；**不要用 Git Bash 的
  heredoc**（会把 `\\` 压成 `\`）。
- 同一文件在多个任务中出现时，每次给出的都是该任务结束时的完整内容，直接整体
  替换；前面任务的版本是为了让每个任务单独可编译、可测试。
- 格式：本计划的文件已按 `rustfmt --edition 2024` 格式化；不要对任何 `lib.rs`
  或 `tracker.rs` 运行 rustfmt（会连带改动其他模块）。
- 不要添加 `use std::future::Future;`（edition 2024 的 prelude 已包含）。
- 在 Git Bash 中运行以 `/` 开头的参数时设 `MSYS_NO_PATHCONV=1`。
- 测试不得连接外部主机，不得读写真实的系统钥匙串、`~/.ssh`、`~/.mai`、
  `~/.claude`、`~/.codex`，也不得连接开发者的 ssh-agent（测试的
  `ConnectOptions.use_agent` 为 `false`）。
- 远程主机（Mac、Ubuntu）只用于测试：**不在远程主机上构建或打包**；
  远程端到端验证使用 GitHub Actions `probes` 产物中的探针。
- 端到端只 attach 到**新建的一次性 session**（`mai-e2e-03e`），结束后
  `kill-session` 与 `delete-session --force` 清理；**绝不** attach 或修改用户的
  `work` 或其他已有 session。Mac 只在用户同意后使用。
- 每次提交信息末尾空一行加：
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
- 分支：`feat/03e-terminal-hardening`，从 `main` 创建；本项目只有一名开发者，
  完成后本地合并到 `main` 并直接推送，不开 PR。

## Review Focus

- **连接断开与单个 channel 关闭要区分**：连接仍在时只重新 attach 那一个终端
  （Task 2 `closed_channel_reattaches_only_that_terminal`），反复关闭则结束
  （`channel_that_keeps_closing_ends_the_terminal`）；连接断开时全部重连
  （`lost_connection_detaches_then_reattaches_with_current_size`）。
- **连接期间（可能在等主机密钥确认）关闭终端或停止主机要立即生效**
  （Task 2 `close_is_handled_while_reconnecting`、`stop_ends_the_task_while_connecting`），
  并发的打开请求排队并共用同一连接（`open_during_connect_waits_its_turn`）。
- **服务器不答复 env 请求**：建立失败并关闭 channel，不能把迟到的答复当成下一个
  请求的答复（Task 1 `unanswered_request_fails_and_closes_the_channel`）。
- **主机移除后以同一 id 重新加入**：旧任务的事件不能落到新主机上（Task 3
  `events_of_an_earlier_incarnation_are_dropped`）。
- **用户自己的 UTF-8 locale（如 `zh_CN.UTF-8`）不能被覆盖**（Task 4
  `local_terminal_keeps_the_users_utf8_locale`）。

## 遗留事项的处理

| 编号 | 处理 |
| --- | --- |
| B26 | 请求无应答即失败；失败时关闭 channel；答复前的输出保留（Task 1） |
| B27 | `alive()` 区分单个 channel 关闭；`STABLE_AFTER` 后才复位退避；打开时返回真实原因（Task 2） |
| B28 | 连接与 attach 期间用 `busy()` 处理命令，打开请求排队（Task 2） |
| B29 | 每次 `add_host` 一个代数，事件按代数过滤（Task 3） |
| B30 | 远程在主机上查找 zellij（常见目录、登录 shell）；本机同样查常见目录（Task 4） |
| B31 | session 名为空或以 `-` 开头时拒绝（Task 2） |
| B32 | 只在没有 UTF-8 locale 时补上，Linux 用 `C.UTF-8`（Task 4） |
| B38、B39、B40 | `use_agent` 选项；旧口令键迁移；`SignError` 的 Display（Task 5） |
| B34、B35、B37 | SFTP 超时 60 秒且超时自动重试；跳板主机 `ProxyJump` 提示；失败时附加提示（Task 6） |
| B36 | 用户决定保持严格（设计 3.3 已注明），不改代码 |
| B25、B33 | 需要观察或 Mac，不在本计划；B41（Ubuntu 终端端到端）记入遗留事项（Task 7） |

## 文件结构

```text
crates/mai-core/
  src/ssh/pty.rs             reply 超时即失败、保留早到输出、失败时关闭 channel
  src/term.rs                busy()、Link::Busy/Down{error}、alive 区分、STABLE_AFTER、
                             check_session、TermError::BadSession、MAX_QUICK_ENDS
  src/terminals.rs           alive；ZELLIJ_DIRS、find_zellij_*、found_zellij、
                             local_zellij、local_env_with、FALLBACK_LOCALE
  src/manager.rs             check_session；代数与转发任务（含单元测试）
  src/ssh/client.rs          is_closed；use_agent；SignError 文本；SFTP 超时
  src/ssh/signer.rs          SignError Display；口令键迁移
  src/ssh/auth.rs            legacy_passphrase_key
  src/ssh/config.rs          jump_warnings
  src/deploy.rs              upload_error（超时可重试）
  src/connect.rs             终端连接查找 zellij；with_notes；jump_warnings
  examples/common/mod.rs     use_agent: true
  tests/ssh_pty.rs、term.rs、hosts.rs、ssh_session.rs、deploy.rs、ssh_config.rs、
  connect.rs；tests/terminals.rs (新)
```

---

### Task 1: SSH PTY 建立（B26）

**Files:**

- Modify: `crates/mai-core/src/ssh/pty.rs`
- Test: `crates/mai-core/tests/ssh_pty.rs`

**Interfaces:**

- Produces（行为）：`SshSession::open_pty` 的 pty、env、exec 请求任一 10 秒
  （`REPLY_TIMEOUT`）内无答复时返回 `SshError::Channel("no answer to the <what>
  request within 10s")`；pty 或 exec 被拒、无答复、channel 关闭时，返回错误前
  先关闭 channel；答复前到达的输出作为第一个 `PtyOut::Data` 交出。签名不变。

- [ ] **Step 1: 写失败测试 `crates/mai-core/tests/ssh_pty.rs`（完整内容）**

测试服务器新增：env 请求的三种答复（接受、拒绝、不答复）、拒绝 pty、
记录 channel 关闭（`close`）、对命令 `early` 先发输出再答复。

```rust
//! `SshSession::open_pty` against an in-process russh server that records
//! channel requests, echoes input, reports resizes and can exit or drop.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mai_core::pty::{PtyIn, PtyIo, PtyOut, TermSize};
use mai_core::ssh::auth::{KbdPrompt, Prompter, Secret, SecretStore};
use mai_core::ssh::client::{ConnectOptions, SshSession, connect};
use mai_core::ssh::config::HostSpec;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;
use tokio::time::timeout;

type Log = Arc<Mutex<Vec<String>>>;

/// How the server answers `env` requests.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Env {
    Accept,
    Refuse,
    /// Never answers.
    Silent,
}

#[derive(Clone)]
struct PtyServer {
    env: Env,
    refuse_pty: bool,
    log: Log,
}

impl PtyServer {
    fn note(&self, s: String) {
        self.log.lock().unwrap().push(s);
    }
}

impl server::Handler for PtyServer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
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

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.note(format!("pty {term} {cols}x{rows}"));
        if self.refuse_pty {
            session.channel_failure(channel)
        } else {
            session.channel_success(channel)
        }
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        value: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match self.env {
            Env::Accept => {
                self.note(format!("env {name}={value}"));
                session.channel_success(channel)
            }
            Env::Refuse => session.channel_failure(channel),
            Env::Silent => Ok(()),
        }
    }

    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.note("close".into());
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.note(format!("exec {}", String::from_utf8_lossy(data)));
        if data == b"early" {
            // Output that arrives before the answer to the request.
            session.data(channel, b"early output\r\n".to_vec())?;
        }
        session.channel_success(channel)?;
        if data == b"two-streams" {
            session.data(channel, b"out-1\n".to_vec())?;
            session.extended_data(channel, 1, b"warn: first\nerror: bad rules\n".to_vec())?;
            session.data(channel, b"out-2\n".to_vec())?;
            session.exit_status_request(channel, 1)?;
            session.eof(channel)?;
            return session.close(channel);
        }
        session.data(channel, b"ready\r\n".to_vec())
    }

    /// `exit` ends with status 5; `drop` closes without a status (lost
    /// connection); anything else is echoed.
    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if data.starts_with(b"exit") {
            session.exit_status_request(channel, 5)?;
            session.eof(channel)?;
            session.close(channel)
        } else if data.starts_with(b"drop") {
            session.close(channel)
        } else {
            session.data(channel, data.to_vec())
        }
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, format!("size={cols}x{rows}\r\n").into_bytes())
    }
}

async fn start(server: PtyServer) -> u16 {
    let key = PrivateKey::random(&mut UnwrapErr(SysRng), Algorithm::Ed25519).unwrap();
    let config = Arc::new(server::Config {
        keys: vec![key],
        methods: MethodSet::from(&[MethodKind::None][..]),
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (config, server) = (config.clone(), server.clone());
            tokio::spawn(async move {
                let _ = server::run_stream(config, stream, server).await;
            });
        }
    });
    port
}

/// Trusts every host key; never has secrets.
struct Trusting;

impl Prompter for Trusting {
    async fn confirm_host_key(&self, _h: &str, _p: u16, _f: &str) -> bool {
        true
    }
    async fn password(&self, _u: &str, _h: &str) -> Option<Secret> {
        None
    }
    async fn passphrase(&self, _k: &Path) -> Option<Secret> {
        None
    }
    async fn keyboard_interactive(
        &self,
        _n: &str,
        _i: &str,
        _p: &[KbdPrompt],
    ) -> Option<Vec<String>> {
        None
    }
}

struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _key: &str) -> Option<String> {
        None
    }
    fn set(&self, _key: &str, _value: &str) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, _key: &str) -> Result<(), String> {
        Ok(())
    }
}

async fn session(accept_env: bool) -> (SshSession<Trusting>, Log, tempfile::TempDir) {
    let env = if accept_env { Env::Accept } else { Env::Refuse };
    session_with(env, false).await
}

async fn session_with(
    env: Env,
    refuse_pty: bool,
) -> (SshSession<Trusting>, Log, tempfile::TempDir) {
    let log = Log::default();
    let port = start(PtyServer {
        env,
        refuse_pty,
        log: log.clone(),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let learn_to = dir.path().join("known_hosts");
    let opts = ConnectOptions {
        known_hosts: vec![learn_to.clone()],
        learn_to,
        timeout: Duration::from_secs(10),
    };
    let spec = HostSpec {
        alias: "pty".into(),
        host: "127.0.0.1".into(),
        port,
        user: "tester".into(),
        identity_files: vec![],
        jumps: vec![],
    };
    let s = connect(&spec, &opts, Arc::new(Trusting), &NoSecrets)
        .await
        .expect("connect");
    (s, log, dir)
}

const ENV: [(&str, &str); 2] = [("LANG", "en_US.UTF-8"), ("LC_CTYPE", "en_US.UTF-8")];

fn command(accepted: bool) -> String {
    if accepted {
        "zellij attach work".into()
    } else {
        "LANG=en_US.UTF-8 LC_CTYPE=en_US.UTF-8 zellij attach work".into()
    }
}

/// Read output until it contains `needle`.
async fn read_until(io: &mut PtyIo, needle: &str) {
    let mut seen = String::new();
    while !seen.contains(needle) {
        match timeout(Duration::from_secs(10), io.output.recv()).await {
            Ok(Some(PtyOut::Data(d))) => seen.push_str(&String::from_utf8_lossy(&d)),
            other => panic!("waiting for {needle:?}, got {other:?} after {seen:?}"),
        }
    }
}

/// The next non-data event, or `None` when output ended.
async fn end(io: &mut PtyIo) -> Option<PtyOut> {
    loop {
        match timeout(Duration::from_secs(10), io.output.recv()).await {
            Ok(Some(PtyOut::Data(_))) => continue,
            Ok(other) => return other,
            Err(_) => panic!("pty did not end"),
        }
    }
}

#[tokio::test]
async fn pty_carries_input_output_resize_and_exit() {
    let (s, log, _dir) = session(true).await;
    let size = TermSize {
        cols: 100,
        rows: 30,
    };
    let mut io = s.open_pty(size, &ENV, command).await.unwrap();
    read_until(&mut io, "ready").await;
    io.input.send(PtyIn::Data(b"hello".to_vec())).unwrap();
    read_until(&mut io, "hello").await;
    io.input
        .send(PtyIn::Resize(TermSize {
            cols: 120,
            rows: 40,
        }))
        .unwrap();
    read_until(&mut io, "size=120x40").await;
    io.input.send(PtyIn::Data(b"exit".to_vec())).unwrap();
    assert_eq!(end(&mut io).await, Some(PtyOut::Exit(Some(5))));
    assert_eq!(
        log.lock().unwrap().clone(),
        vec![
            "pty xterm-256color 100x30",
            "env LANG=en_US.UTF-8",
            "env LC_CTYPE=en_US.UTF-8",
            "exec zellij attach work",
        ]
    );
}

#[tokio::test]
async fn refused_env_falls_back_to_command_prefix() {
    let (s, log, _dir) = session(false).await;
    let size = TermSize { cols: 80, rows: 24 };
    let mut io = s.open_pty(size, &ENV, command).await.unwrap();
    read_until(&mut io, "ready").await;
    assert_eq!(
        log.lock().unwrap().last().cloned(),
        Some("exec LANG=en_US.UTF-8 LC_CTYPE=en_US.UTF-8 zellij attach work".to_owned())
    );
}

#[tokio::test]
async fn channel_closed_without_exit_status_is_a_lost_connection() {
    let (s, _log, _dir) = session(true).await;
    let size = TermSize { cols: 80, rows: 24 };
    let mut io = s.open_pty(size, &ENV, command).await.unwrap();
    read_until(&mut io, "ready").await;
    io.input.send(PtyIn::Data(b"drop".to_vec())).unwrap();
    assert_eq!(end(&mut io).await, None, "no Exit: the connection was lost");
}

/// Wait until the server has seen the channel close.
async fn closed(log: &Log) {
    timeout(Duration::from_secs(10), async {
        while !log.lock().unwrap().iter().any(|l| l == "close") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("channel closed on the server");
}

#[tokio::test]
async fn refused_pty_closes_the_channel() {
    let (s, log, _dir) = session_with(Env::Accept, true).await;
    let size = TermSize { cols: 80, rows: 24 };
    let err = s.open_pty(size, &ENV, command).await.err().unwrap();
    assert!(err.to_string().contains("refused the pty"), "{err}");
    closed(&log).await;
}

/// A server that never answers fails the setup (instead of taking a late
/// answer for the next request's) and the channel is closed.
#[tokio::test]
async fn unanswered_request_fails_and_closes_the_channel() {
    let (s, log, _dir) = session_with(Env::Silent, false).await;
    let size = TermSize { cols: 80, rows: 24 };
    let err = s.open_pty(size, &ENV, command).await.err().unwrap();
    assert!(
        err.to_string().contains("no answer to the env request"),
        "{err}"
    );
    closed(&log).await;
    assert!(
        !log.lock().unwrap().iter().any(|l| l.starts_with("exec")),
        "no command after an unanswered request"
    );
}

#[tokio::test]
async fn output_before_the_command_is_accepted_is_kept() {
    let (s, _log, _dir) = session(true).await;
    let size = TermSize { cols: 80, rows: 24 };
    let mut io = s.open_pty(size, &ENV, |_| "early".into()).await.unwrap();
    read_until(&mut io, "early output").await;
}

#[tokio::test]
async fn exec_split_separates_stdout_from_stderr() {
    use mai_core::stderr::StderrTail;
    use tokio::io::AsyncReadExt;

    let (s, _log, _dir) = session(true).await;
    let tail = Arc::new(StderrTail::default());
    let (mut out, _stdin) = s
        .open_exec_split("two-streams", &[], tail.clone())
        .await
        .unwrap();
    let mut text = String::new();
    timeout(Duration::from_secs(10), out.read_to_string(&mut text))
        .await
        .expect("stdout ends")
        .unwrap();
    assert_eq!(text, "out-1\nout-2\n");
    assert_eq!(
        tail.summary().await.as_deref(),
        Some("warn: first | error: bad rules")
    );
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test ssh_pty`
Expected: `refused_pty_closes_the_channel` 等新测试失败（channel 未关闭、
早到输出丢失；`unanswered_request_fails_and_closes_the_channel` 约 10 秒后失败）。

- [ ] **Step 3: 写 `crates/mai-core/src/ssh/pty.rs`（完整内容）**

```rust
//! A PTY on an SSH session channel, bridged to `PtyIo`.

use std::time::Duration;

use russh::client;
use russh::{Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf};

use super::client::SshError;
use crate::pty::{PtyEnds, PtyIn, PtyIo, PtyOut, TermSize, pty_channels};

/// How long to wait for the server to answer a channel request.
pub const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

type Writer = ChannelWriteHalf<client::Msg>;

fn chan_err(e: impl std::fmt::Display) -> SshError {
    SshError::Channel(e.to_string())
}

/// The server's answer to the last request: `true` accepted, `false`
/// refused. Output that arrives first is kept in `early`. No answer in
/// time is an error: a late answer would otherwise be taken for the answer
/// to the next request.
async fn reply(
    read: &mut ChannelReadHalf,
    early: &mut Vec<u8>,
    what: &str,
) -> Result<bool, SshError> {
    let wait = async {
        loop {
            match read.wait().await {
                Some(ChannelMsg::Success) => return Ok(true),
                Some(ChannelMsg::Failure) => return Ok(false),
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    early.extend_from_slice(&data);
                }
                Some(ChannelMsg::Close) | None => {
                    return Err(SshError::Channel("channel closed".into()));
                }
                Some(_) => {}
            }
        }
    };
    match tokio::time::timeout(REPLY_TIMEOUT, wait).await {
        Ok(r) => r,
        Err(_) => Err(SshError::Channel(format!(
            "no answer to the {what} request within {}s",
            REPLY_TIMEOUT.as_secs()
        ))),
    }
}

/// Request the PTY, the variables and the command; output seen before the
/// command was accepted is returned.
async fn setup(
    read: &mut ChannelReadHalf,
    write: &Writer,
    size: TermSize,
    env: &[(&str, &str)],
    command: impl FnOnce(bool) -> String,
) -> Result<Vec<u8>, SshError> {
    let mut early = Vec::new();
    let (cols, rows) = (u32::from(size.cols), u32::from(size.rows));
    write
        .request_pty(true, "xterm-256color", cols, rows, 0, 0, &[])
        .await
        .map_err(chan_err)?;
    if !reply(read, &mut early, "pty").await? {
        return Err(SshError::Channel("server refused the pty request".into()));
    }
    let mut accepted = true;
    for (k, v) in env {
        write.set_env(true, *k, *v).await.map_err(chan_err)?;
        accepted &= reply(read, &mut early, "env").await?;
    }
    write
        .exec(true, command(accepted))
        .await
        .map_err(chan_err)?;
    if !reply(read, &mut early, "exec").await? {
        return Err(SshError::Channel("server refused the command".into()));
    }
    Ok(early)
}

/// Request a PTY of `size` and the variables in `env` on `ch`, then run
/// `command(env_accepted)`, where `env_accepted` says whether the server
/// accepted every variable (so the caller can fall back to a command
/// prefix). The returned `PtyIo` is driven by a spawned task. If setting
/// up fails, the channel is closed.
pub(crate) async fn start(
    ch: Channel<client::Msg>,
    size: TermSize,
    env: &[(&str, &str)],
    command: impl FnOnce(bool) -> String,
) -> Result<PtyIo, SshError> {
    let (mut read, write) = ch.split();
    let early = match setup(&mut read, &write, size, env, command).await {
        Ok(early) => early,
        Err(e) => {
            let _ = write.close().await;
            return Err(e);
        }
    };
    let (io, ends) = pty_channels();
    if !early.is_empty() {
        let _ = ends.output.send(PtyOut::Data(early));
    }
    tokio::spawn(pump(read, write, ends));
    Ok(io)
}

/// Move bytes both ways until the server closes the channel. `Exit` is
/// sent only when the program's exit was reported; a channel that ends
/// without it (connection lost) just ends the output.
async fn pump(mut read: ChannelReadHalf, write: Writer, ends: PtyEnds) {
    let PtyEnds { mut input, output } = ends;
    let mut status = None;
    let mut exited = false;
    let mut input_open = true;
    loop {
        tokio::select! {
            msg = read.wait() => match msg {
                Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                    if output.send(PtyOut::Data(data.to_vec())).is_err() {
                        let _ = write.close().await;
                        return;
                    }
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    status = Some(exit_status);
                    exited = true;
                }
                Some(ChannelMsg::ExitSignal { .. }) => exited = true,
                Some(ChannelMsg::Close) | None => break,
                Some(_) => {}
            },
            cmd = input.recv(), if input_open => match cmd {
                Some(PtyIn::Data(d)) => {
                    if write.data(&d[..]).await.is_err() {
                        break;
                    }
                }
                Some(PtyIn::Resize(s)) => {
                    let _ = write
                        .window_change(u32::from(s.cols), u32::from(s.rows), 0, 0)
                        .await;
                }
                Some(PtyIn::Close) | None => {
                    input_open = false;
                    let _ = write.eof().await;
                    let _ = write.close().await;
                }
            },
        }
    }
    if exited {
        let _ = output.send(PtyOut::Exit(status));
    }
}
```

- [ ] **Step 4: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: `ssh_pty` 7 个测试通过（其中一个约 10 秒），全部 253 个；clippy 无警告。

- [ ] **Step 5: 提交**

```bash
git add crates/mai-core
git commit -m "fix(core): fail PTY setup on an unanswered request and close the channel

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: 终端任务（B27、B28、B31）

**Files:**

- Modify: `crates/mai-core/src/term.rs`、`crates/mai-core/src/terminals.rs`、
  `crates/mai-core/src/ssh/client.rs`、`crates/mai-core/src/manager.rs`
- Test: `crates/mai-core/tests/term.rs`、`crates/mai-core/tests/hosts.rs`

**Interfaces:**

- Consumes: `host::STABLE_AFTER`（已有，30 秒）。
- Produces:

  ```rust
  // term
  pub trait TermTransport: Send + 'static {
      fn attach(&mut self, zellij: Option<&str>, spec: &AttachSpec)
          -> impl Future<Output = Result<PtyIo, OpenError>> + Send;
      fn alive(&self) -> bool;                    // 新增：连接本身是否还在
  }
  pub const MAX_QUICK_ENDS: u32 = 3;
  pub enum TermError { NoHost, BadSession(String), Open(OpenError), Stopped }
  pub fn check_session(name: &str) -> Result<(), TermError>
  // ssh::client
  impl SshSession<P> { pub fn is_closed(&self) -> bool }
  ```

  `run_terms` 签名不变。`HostManager::open_terminal` 先调用 `check_session`。
  单个终端输出结束且没有 `Exit` 时：`alive()` 为真则只重新 attach 该终端
  （先发 `Detached { reason: "terminal channel closed" }`），连续
  `MAX_QUICK_ENDS` 次在 attach 后 `STABLE_AFTER` 内关闭则发 `Exited(None)`；
  `alive()` 为假则按连接丢失处理。

- [ ] **Step 1: 写失败测试**

`crates/mai-core/tests/term.rs`（完整内容；假连接带 `alive` 标志与可阻塞连接的
`gate`）：

```rust
//! Terminal task against a scripted connector whose PTYs the test drives,
//! plus the attach command lines. Time is paused: backoff waits pass
//! instantly.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mai_core::deploy::{Os, Remote, Shell};
use mai_core::host::{
    ConnState, Connector, HostConfig, HostEvent, HostKind, OpenError, Opened, Problem, STABLE_AFTER,
};
use mai_core::pty::{PtyEnds, PtyIn, PtyIo, PtyOut, TermSize, pty_channels};
use mai_core::term::{
    AttachSpec, MAX_QUICK_ENDS, OpenResult, TermCmd, TermError, TermEvent, TermTransport,
    ZellijPaths, attach_argv, check_session, remote_attach_command, run_terms,
};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::timeout;

const SIZE: TermSize = TermSize { cols: 80, rows: 24 };

fn remote(os: Os, shell: Shell) -> Remote {
    Remote {
        os,
        arch: "x86_64".into(),
        shell,
        home: "h".into(),
    }
}

#[test]
fn attach_arguments() {
    assert_eq!(attach_argv("work", false), vec!["attach", "work"]);
    assert_eq!(attach_argv("new", true), vec!["attach", "--create", "new"]);
}

#[test]
fn session_names_zellij_would_misread_are_refused() {
    assert!(check_session("work").is_ok());
    assert!(check_session("a-b").is_ok());
    assert_eq!(check_session("-h"), Err(TermError::BadSession("-h".into())));
    assert_eq!(check_session(""), Err(TermError::BadSession(String::new())));
    let msg = TermError::BadSession("--create".into()).to_string();
    assert!(msg.contains("start with '-'"), "{msg}");
}

#[test]
fn remote_attach_command_per_shell_and_locale() {
    let posix = remote(Os::MacOs, Shell::Posix);
    assert_eq!(
        remote_attach_command(
            &posix,
            Some("/opt/homebrew/bin/zellij"),
            "work",
            false,
            true
        ),
        "'/opt/homebrew/bin/zellij' attach work"
    );
    assert_eq!(
        remote_attach_command(&posix, None, "my session", true, false),
        "LANG=en_US.UTF-8 LC_CTYPE=en_US.UTF-8 'zellij' attach --create 'my session'"
    );
    let cmd = remote(Os::Windows, Shell::Cmd);
    assert_eq!(
        remote_attach_command(&cmd, Some(r"D:\Apps\zellij\zellij.exe"), "w", false, false),
        r#""D:\Apps\zellij\zellij.exe" attach w"#
    );
    let ps = remote(Os::Windows, Shell::PowerShell);
    assert_eq!(
        remote_attach_command(&ps, None, "w", false, false),
        "& 'zellij' attach w"
    );
}

/// One `attach` the terminal task made, and the PTY ends to drive it.
struct Attached {
    spec: AttachSpec,
    zellij: Option<String>,
    ends: PtyEnds,
}

struct FakeTerms {
    attaches: UnboundedSender<Attached>,
    alive: Arc<AtomicBool>,
}

impl TermTransport for FakeTerms {
    async fn attach(
        &mut self,
        zellij: Option<&str>,
        spec: &AttachSpec,
    ) -> Result<PtyIo, OpenError> {
        let (io, ends) = pty_channels();
        let a = Attached {
            spec: spec.clone(),
            zellij: zellij.map(str::to_owned),
            ends,
        };
        self.attaches.send(a).unwrap();
        Ok(io)
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
}

/// `open_terminals` waits for `gate`, then fails with the queued errors
/// first, then succeeds.
struct FakeConnector {
    failures: Mutex<VecDeque<OpenError>>,
    opens: AtomicUsize,
    attaches: UnboundedSender<Attached>,
    /// Liveness of the latest connection.
    alive: Mutex<Arc<AtomicBool>>,
    gate: Arc<tokio::sync::Mutex<()>>,
}

impl Connector for FakeConnector {
    type Terminals = FakeTerms;

    async fn open(&self, _host: &HostConfig) -> Result<Opened, OpenError> {
        std::future::pending().await
    }

    async fn open_terminals(&self, _host: &HostConfig) -> Result<FakeTerms, OpenError> {
        drop(self.gate.lock().await);
        self.opens.fetch_add(1, Ordering::SeqCst);
        if let Some(e) = self.failures.lock().unwrap().pop_front() {
            return Err(e);
        }
        let alive = Arc::new(AtomicBool::new(true));
        *self.alive.lock().unwrap() = alive.clone();
        Ok(FakeTerms {
            attaches: self.attaches.clone(),
            alive,
        })
    }
}

struct Harness {
    connector: Arc<FakeConnector>,
    cmds: UnboundedSender<TermCmd>,
    events: UnboundedReceiver<(String, HostEvent)>,
    attaches: UnboundedReceiver<Attached>,
    zellij: ZellijPaths,
    task: tokio::task::JoinHandle<()>,
}

fn start(zellij_cfg: Option<&str>, failures: Vec<OpenError>) -> Harness {
    let (attach_tx, attaches) = mpsc::unbounded_channel();
    let connector = Arc::new(FakeConnector {
        failures: Mutex::new(failures.into()),
        opens: AtomicUsize::new(0),
        attaches: attach_tx,
        alive: Mutex::new(Arc::new(AtomicBool::new(true))),
        gate: Arc::default(),
    });
    let (ev_tx, events) = mpsc::unbounded_channel();
    let (cmds, cmd_rx) = mpsc::unbounded_channel();
    let zellij = ZellijPaths::default();
    let cfg = HostConfig {
        id: "h".into(),
        kind: HostKind::Local,
        zellij: zellij_cfg.map(str::to_owned),
    };
    let task = tokio::spawn(run_terms(
        cfg,
        connector.clone(),
        zellij.clone(),
        ev_tx,
        cmd_rx,
    ));
    Harness {
        connector,
        cmds,
        events,
        attaches,
        zellij,
        task,
    }
}

fn spec(session: &str) -> AttachSpec {
    AttachSpec {
        session: session.into(),
        create: false,
        size: SIZE,
    }
}

impl Harness {
    /// Ask to open a terminal; the answer comes on the returned receiver.
    fn request(&self, session: &str) -> oneshot::Receiver<OpenResult> {
        let (reply, answer) = oneshot::channel();
        let spec = spec(session);
        self.cmds.send(TermCmd::Open { spec, reply }).unwrap();
        answer
    }

    async fn open(&self, session: &str) -> OpenResult {
        self.request(session).await.unwrap()
    }

    async fn attached(&mut self) -> Attached {
        timeout(Duration::from_secs(600), self.attaches.recv())
            .await
            .expect("attach")
            .unwrap()
    }

    /// Next terminal connection state.
    async fn state(&mut self) -> Option<ConnState> {
        loop {
            let (_, ev) = timeout(Duration::from_secs(600), self.events.recv())
                .await
                .expect("host event")
                .unwrap();
            if let HostEvent::Term(s) = ev {
                return s;
            }
        }
    }

    /// The connection drops: the transport reports itself dead and the
    /// PTY output of `pty` ends without an exit status.
    fn lose_connection(&self, pty: Attached) {
        self.connector
            .alive
            .lock()
            .unwrap()
            .store(false, Ordering::SeqCst);
        drop(pty.ends.output);
    }
}

async fn next(rx: &mut UnboundedReceiver<TermEvent>) -> Option<TermEvent> {
    timeout(Duration::from_secs(600), rx.recv())
        .await
        .expect("terminal event")
}

fn retry_in(s: Option<ConnState>) -> Duration {
    match s {
        Some(ConnState::Retrying { retry_in, .. }) => retry_in,
        other => panic!("{other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn terminal_relays_both_ways_and_ends_on_exit() {
    let mut h = start(None, vec![]);
    let (id, mut rx) = h.open("work").await.unwrap();
    assert_eq!(h.state().await, Some(ConnState::Up));
    let mut pty = h.attached().await;
    assert_eq!(pty.spec.session, "work");
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));

    h.cmds
        .send(TermCmd::Input {
            id,
            data: b"ls\r".to_vec(),
        })
        .unwrap();
    assert_eq!(
        pty.ends.input.recv().await,
        Some(PtyIn::Data(b"ls\r".to_vec()))
    );
    let bigger = TermSize {
        cols: 120,
        rows: 40,
    };
    h.cmds.send(TermCmd::Resize { id, size: bigger }).unwrap();
    assert_eq!(pty.ends.input.recv().await, Some(PtyIn::Resize(bigger)));

    pty.ends.output.send(PtyOut::Data(b"hi".to_vec())).unwrap();
    assert_eq!(next(&mut rx).await, Some(TermEvent::Output(b"hi".to_vec())));
    pty.ends.output.send(PtyOut::Exit(Some(0))).unwrap();
    assert_eq!(next(&mut rx).await, Some(TermEvent::Exited(Some(0))));
    assert_eq!(
        next(&mut rx).await,
        None,
        "finished terminal ends its stream"
    );
    assert_eq!(h.state().await, None, "last terminal closes the connection");
}

#[tokio::test(start_paused = true)]
async fn zellij_from_config_wins_over_probe_report() {
    let mut h = start(Some("/cfg/zellij"), vec![]);
    h.zellij
        .lock()
        .unwrap()
        .insert("h".into(), "/probe/zellij".into());
    h.open("w").await.unwrap();
    assert_eq!(h.attached().await.zellij.as_deref(), Some("/cfg/zellij"));

    let mut h = start(None, vec![]);
    h.open("w").await.unwrap();
    assert_eq!(h.attached().await.zellij, None);
    h.zellij
        .lock()
        .unwrap()
        .insert("h".into(), "/probe/zellij".into());
    h.open("w2").await.unwrap();
    assert_eq!(h.attached().await.zellij.as_deref(), Some("/probe/zellij"));
}

#[tokio::test(start_paused = true)]
async fn lost_connection_detaches_then_reattaches_with_current_size() {
    let mut h = start(None, vec![]);
    let (id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    let first = h.attached().await;
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));
    let bigger = TermSize {
        cols: 132,
        rows: 50,
    };
    h.cmds.send(TermCmd::Resize { id, size: bigger }).unwrap();

    h.lose_connection(first);
    match next(&mut rx).await {
        Some(TermEvent::Detached { reason }) => assert!(reason.contains("lost"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(retry_in(h.state().await), Duration::from_secs(1));
    assert_eq!(h.state().await, Some(ConnState::Up));
    let second = h.attached().await;
    assert_eq!(second.spec.session, "work");
    assert_eq!(second.spec.size, bigger);
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));
    assert_eq!(h.connector.opens.load(Ordering::SeqCst), 2);

    // Output of the new PTY flows; the old one is ignored.
    second
        .ends
        .output
        .send(PtyOut::Data(b"again".to_vec()))
        .unwrap();
    assert_eq!(
        next(&mut rx).await,
        Some(TermEvent::Output(b"again".to_vec()))
    );
}

/// B27: one terminal's channel closing while the connection is fine
/// reattaches that terminal only, over the same connection.
#[tokio::test(start_paused = true)]
async fn closed_channel_reattaches_only_that_terminal() {
    let mut h = start(None, vec![]);
    let (_a, mut rx_a) = h.open("a").await.unwrap();
    h.state().await;
    let pty_a = h.attached().await;
    let (_b, mut rx_b) = h.open("b").await.unwrap();
    let _pty_b = h.attached().await;
    assert_eq!(next(&mut rx_a).await, Some(TermEvent::Attached));
    assert_eq!(next(&mut rx_b).await, Some(TermEvent::Attached));

    drop(pty_a.ends.output); // channel closed, connection still alive
    assert_eq!(
        next(&mut rx_a).await,
        Some(TermEvent::Detached {
            reason: "terminal channel closed".into()
        })
    );
    assert_eq!(h.attached().await.spec.session, "a");
    assert_eq!(next(&mut rx_a).await, Some(TermEvent::Attached));
    assert_eq!(
        h.connector.opens.load(Ordering::SeqCst),
        1,
        "same connection"
    );
    assert!(rx_b.try_recv().is_err(), "the other terminal is untouched");
}

#[tokio::test(start_paused = true)]
async fn channel_that_keeps_closing_ends_the_terminal() {
    let mut h = start(None, vec![]);
    let (_id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));
    for _ in 1..MAX_QUICK_ENDS {
        drop(h.attached().await.ends.output);
        assert!(matches!(
            next(&mut rx).await,
            Some(TermEvent::Detached { .. })
        ));
        assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));
    }
    drop(h.attached().await.ends.output);
    assert_eq!(next(&mut rx).await, Some(TermEvent::Exited(None)));
    assert_eq!(next(&mut rx).await, None);
    assert_eq!(h.state().await, None, "last terminal closes the connection");
}

/// B27: the backoff only starts over after the connection stayed up for
/// `STABLE_AFTER`, so a connection that drops right after every reconnect
/// does not reconnect every second.
#[tokio::test(start_paused = true)]
async fn backoff_starts_over_only_after_a_stable_connection() {
    let mut h = start(None, vec![]);
    let (_id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    let pty = h.attached().await;
    next(&mut rx).await;

    h.lose_connection(pty);
    assert_eq!(retry_in(h.state().await), Duration::from_secs(1));
    assert_eq!(h.state().await, Some(ConnState::Up));
    let pty = h.attached().await;
    h.lose_connection(pty);
    assert_eq!(retry_in(h.state().await), Duration::from_secs(2));
    assert_eq!(h.state().await, Some(ConnState::Up));
    let pty = h.attached().await;

    tokio::time::sleep(STABLE_AFTER).await;
    h.lose_connection(pty);
    assert_eq!(retry_in(h.state().await), Duration::from_secs(1));
}

#[tokio::test(start_paused = true)]
async fn closing_the_last_terminal_detaches_and_closes_the_connection() {
    let mut h = start(None, vec![]);
    let (id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    let mut pty = h.attached().await;
    h.cmds.send(TermCmd::Close { id }).unwrap();
    assert_eq!(pty.ends.input.recv().await, Some(PtyIn::Close));
    assert_eq!(h.state().await, None);
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));
    assert_eq!(next(&mut rx).await, None);
}

#[tokio::test(start_paused = true)]
async fn failed_open_is_reported_to_the_caller() {
    let mut h = start(
        None,
        vec![OpenError::NeedsUser(Problem::Auth("denied".into()))],
    );
    let r = h.open("work").await;
    assert_eq!(
        r.err(),
        Some(TermError::Open(OpenError::NeedsUser(Problem::Auth(
            "denied".into()
        ))))
    );
    assert_eq!(h.state().await, None);
    assert!(h.open("work").await.is_ok(), "next open tries again");
}

#[tokio::test(start_paused = true)]
async fn reconnect_needing_the_user_waits_for_retry() {
    let mut h = start(None, vec![]);
    let (_id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    let first = h.attached().await;
    next(&mut rx).await;
    h.connector
        .failures
        .lock()
        .unwrap()
        .push_back(OpenError::NeedsUser(Problem::HostKeyRejected));
    h.lose_connection(first);
    assert!(matches!(h.state().await, Some(ConnState::Retrying { .. })));
    assert_eq!(
        h.state().await,
        Some(ConnState::NeedsUser(Problem::HostKeyRejected))
    );
    tokio::time::sleep(Duration::from_secs(3600)).await;
    assert_eq!(
        h.connector.opens.load(Ordering::SeqCst),
        2,
        "no automatic retry"
    );
    h.cmds.send(TermCmd::Retry).unwrap();
    assert_eq!(h.state().await, Some(ConnState::Up));
    h.attached().await;
}

/// B27: opening a terminal while the connection is down reports why it is
/// down, not just that it is.
#[tokio::test(start_paused = true)]
async fn opening_while_down_reports_the_real_reason() {
    let mut h = start(None, vec![]);
    let (_id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    let first = h.attached().await;
    next(&mut rx).await;
    {
        let mut failures = h.connector.failures.lock().unwrap();
        failures.push_back(OpenError::NeedsUser(Problem::HostKeyRejected));
        failures.push_back(OpenError::NeedsUser(Problem::HostKeyRejected));
    }
    h.lose_connection(first);
    h.state().await; // Retrying
    assert_eq!(
        h.state().await,
        Some(ConnState::NeedsUser(Problem::HostKeyRejected))
    );
    assert_eq!(
        h.open("logs").await.err(),
        Some(TermError::Open(OpenError::NeedsUser(
            Problem::HostKeyRejected
        )))
    );
}

#[tokio::test(start_paused = true)]
async fn opening_a_terminal_while_detached_reattaches_the_others() {
    let mut h = start(None, vec![]);
    let (_id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    let first = h.attached().await;
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));

    h.lose_connection(first);
    assert!(matches!(
        next(&mut rx).await,
        Some(TermEvent::Detached { .. })
    ));
    // Before the backoff fires: opening reconnects and reattaches all.
    let (_id2, _rx2) = h.open("logs").await.unwrap();
    assert_eq!(next(&mut rx).await, Some(TermEvent::Attached));
    let a = h.attached().await;
    let b = h.attached().await;
    let mut sessions = vec![a.spec.session, b.spec.session];
    sessions.sort();
    assert_eq!(sessions, vec!["logs", "work"]);
}

/// B28: while the connection is being made (e.g. a host-key prompt is
/// open), closing a detached terminal takes effect at once and it is not
/// attached again afterwards.
#[tokio::test(start_paused = true)]
async fn close_is_handled_while_reconnecting() {
    let mut h = start(None, vec![]);
    let (id, mut rx) = h.open("work").await.unwrap();
    h.state().await;
    let first = h.attached().await;
    next(&mut rx).await;

    let hold = h.connector.gate.clone().lock_owned().await;
    h.lose_connection(first);
    h.state().await; // Retrying
    tokio::time::sleep(Duration::from_secs(2)).await; // reconnect is waiting
    h.cmds.send(TermCmd::Close { id }).unwrap();
    assert!(matches!(
        next(&mut rx).await,
        Some(TermEvent::Detached { .. })
    ));
    assert_eq!(next(&mut rx).await, None, "closed while reconnecting");
    drop(hold);
    assert_eq!(h.state().await, Some(ConnState::Up));
    assert_eq!(h.state().await, None, "nothing left to attach");
    assert!(h.attaches.try_recv().is_err());
}

/// B28: stop ends the task even while it waits for the connection; the
/// pending open is answered with an error.
#[tokio::test(start_paused = true)]
async fn stop_ends_the_task_while_connecting() {
    let h = start(None, vec![]);
    let hold = h.connector.gate.clone().lock_owned().await;
    let answer = h.request("work");
    let queued = h.request("logs");
    tokio::time::sleep(Duration::from_secs(1)).await;
    h.cmds.send(TermCmd::Stop).unwrap();
    timeout(Duration::from_secs(60), h.task)
        .await
        .expect("task ended")
        .unwrap();
    assert!(answer.await.is_err(), "open answered by dropping it");
    assert!(queued.await.is_err());
    drop(hold);
}

/// B28: an open that arrives while another is connecting waits its turn
/// and uses the same connection.
#[tokio::test(start_paused = true)]
async fn open_during_connect_waits_its_turn() {
    let mut h = start(None, vec![]);
    let hold = h.connector.gate.clone().lock_owned().await;
    let first = h.request("work");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let second = h.request("logs");
    drop(hold);
    assert!(first.await.unwrap().is_ok());
    assert!(second.await.unwrap().is_ok());
    assert_eq!(h.attached().await.spec.session, "work");
    assert_eq!(h.attached().await.spec.session, "logs");
    assert_eq!(h.connector.opens.load(Ordering::SeqCst), 1);
}
```

`crates/mai-core/tests/hosts.rs`（完整内容；`ScriptedTerms` 实现 `alive`，新增
`session_names_starting_with_a_dash_are_refused`）：

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

    fn alive(&self) -> bool {
        true
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

/// B31: a session name zellij would take for an option is refused before
/// anything is opened.
#[tokio::test(start_paused = true)]
async fn session_names_starting_with_a_dash_are_refused() {
    let (c, _probes) = Scripted::new(vec![]);
    let (mut mgr, _rx) = HostManager::start(c.clone(), TrackerConfig::default());
    mgr.add_host(cfg("h"));
    let size = TermSize { cols: 80, rows: 24 };
    let r = mgr.open_terminal("h", "--help", false, size).await;
    assert_eq!(r.err(), Some(TermError::BadSession("--help".into())));
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test term --test hosts`
Expected: 编译失败（`alive`、`MAX_QUICK_ENDS`、`check_session`、`BadSession` 不存在）。

- [ ] **Step 3: 写 `crates/mai-core/src/term.rs`（完整内容）**

```rust
//! Interactive terminals: `zellij attach <session>` in a PTY per open
//! terminal, over a host's terminal connection (TermConn, design 2.2).
//!
//! One task per host owns the terminal connection and all its PTYs. The
//! connection is opened with the first terminal and closed with the last.
//! When it is lost, every terminal is told it is detached, the task
//! reconnects with backoff and attaches each terminal again (zellij keeps
//! the session, so the user continues where they were). While connecting
//! or attaching, the task keeps handling input, resizes, closes and stop;
//! further opens wait their turn.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::deploy::{Remote, Shell};
use crate::host::{Backoff, ConnState, Connector, HostConfig, HostEvent, OpenError, STABLE_AFTER};
use crate::pty::{PtyIn, PtyIo, PtyOut, TermSize};

/// Locale requested for terminals (design 7): without it zellij sessions
/// created over SSH on macOS run in the C locale and CJK input breaks.
pub const LOCALE_ENV: [(&str, &str); 2] = [("LANG", "en_US.UTF-8"), ("LC_CTYPE", "en_US.UTF-8")];

/// A terminal whose channel keeps closing soon after attaching is given up
/// on after this many such closes in a row.
pub const MAX_QUICK_ENDS: u32 = 3;

/// Arguments after the zellij binary: `attach [--create] <session>`.
pub fn attach_argv(session: &str, create: bool) -> Vec<String> {
    let mut argv = vec!["attach".to_owned()];
    if create {
        argv.push("--create".to_owned());
    }
    argv.push(session.to_owned());
    argv
}

/// Command line that attaches to `session` on `remote`. `zellij` is the
/// binary when known (else `zellij` from PATH). When the server refused
/// the locale variables, a POSIX command sets them itself.
pub fn remote_attach_command(
    remote: &Remote,
    zellij: Option<&str>,
    session: &str,
    create: bool,
    env_accepted: bool,
) -> String {
    let cmd = remote.invoke(
        zellij.unwrap_or("zellij"),
        &remote.join_args(&attach_argv(session, create)),
    );
    if env_accepted || remote.shell != Shell::Posix {
        cmd
    } else {
        let prefix: Vec<String> = LOCALE_ENV.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!("{} {cmd}", prefix.join(" "))
    }
}

/// What to attach to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachSpec {
    pub session: String,
    /// Create the session if it does not exist (`attach --create`).
    pub create: bool,
    pub size: TermSize,
}

/// A host's terminal connection: starts `zellij attach` in PTYs.
pub trait TermTransport: Send + 'static {
    /// `zellij` is the binary to run when known.
    fn attach(
        &mut self,
        zellij: Option<&str>,
        spec: &AttachSpec,
    ) -> impl Future<Output = Result<PtyIo, OpenError>> + Send;

    /// Whether the connection itself is still up. When one terminal's
    /// output ends without an exit status, this tells a closed channel
    /// (reattach that terminal) from a lost connection (reattach all).
    fn alive(&self) -> bool;
}

/// Sent to the owner of a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    Output(Vec<u8>),
    /// Attached (again): zellij redraws the whole screen.
    Attached,
    /// The connection (or this terminal's channel) was lost; the terminal
    /// is reattached when possible.
    Detached {
        reason: String,
    },
    /// `zellij attach` exited (the user detached or quit the session), or
    /// its channel kept closing right after attaching. The terminal is
    /// finished.
    Exited(Option<u32>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermError {
    /// No host with this id.
    NoHost,
    /// The session name cannot be passed to zellij (empty, or starts
    /// with `-` and would be taken for an option).
    BadSession(String),
    /// The terminal connection or `zellij attach` could not be started.
    Open(OpenError),
    /// The host was removed while opening.
    Stopped,
}

impl fmt::Display for TermError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoHost => f.write_str("no such host"),
            Self::BadSession(name) => write!(
                f,
                "invalid session name {name:?}: it must not be empty or start with '-'"
            ),
            Self::Open(OpenError::Retry(m)) => write!(f, "terminal connection failed: {m}"),
            Self::Open(OpenError::NeedsUser(p)) => write!(f, "{p}"),
            Self::Stopped => f.write_str("host removed"),
        }
    }
}

impl std::error::Error for TermError {}

/// Session names zellij would misread are refused up front.
pub fn check_session(name: &str) -> Result<(), TermError> {
    if name.is_empty() || name.starts_with('-') {
        return Err(TermError::BadSession(name.to_owned()));
    }
    Ok(())
}

pub type OpenResult = Result<(u64, UnboundedReceiver<TermEvent>), TermError>;

pub enum TermCmd {
    Open {
        spec: AttachSpec,
        reply: oneshot::Sender<OpenResult>,
    },
    Input {
        id: u64,
        data: Vec<u8>,
    },
    Resize {
        id: u64,
        size: TermSize,
    },
    Close {
        id: u64,
    },
    /// Reconnect now (after a problem that needs the user, or to skip a
    /// backoff wait).
    Retry,
    Stop,
}

/// zellij binaries reported by each host's probe (`Hello`), by host id.
pub type ZellijPaths = Arc<Mutex<HashMap<String, String>>>;

struct Slot {
    spec: AttachSpec,
    events: UnboundedSender<TermEvent>,
    /// `None` while detached.
    pty: Option<UnboundedSender<PtyIn>>,
    /// Bumped on every attach, so output of an older PTY is ignored.
    generation: u64,
    attached_at: Instant,
    /// Channel closes in a row that came soon after attaching.
    quick_ends: u32,
}

/// Output of one PTY, tagged with its terminal and attach generation;
/// `None` means the output ended.
type Tagged = (u64, u64, Option<PtyOut>);

enum Link<T> {
    /// No terminals, no connection.
    Idle,
    Up(T),
    /// Lent to an attach that is under way.
    Busy,
    /// Lost; reconnect at `at` (or on `Retry` if `None`). `error` is why,
    /// for terminals opened meanwhile.
    Down {
        at: Option<Instant>,
        error: OpenError,
    },
}

struct Terms<C: Connector> {
    cfg: HostConfig,
    connector: Arc<C>,
    zellij: ZellijPaths,
    events: UnboundedSender<(String, HostEvent)>,
    cmds: UnboundedReceiver<TermCmd>,
    link: Link<C::Terminals>,
    slots: HashMap<u64, Slot>,
    /// Opens that arrived while connecting or attaching.
    pending: VecDeque<(AttachSpec, oneshot::Sender<OpenResult>)>,
    next_id: u64,
    backoff: Backoff,
    /// When the current connection came up.
    up_since: Option<Instant>,
    out_tx: UnboundedSender<Tagged>,
}

fn reason_of(e: OpenError) -> String {
    match e {
        OpenError::Retry(m) => m,
        OpenError::NeedsUser(p) => p.to_string(),
    }
}

impl<C: Connector> Terms<C> {
    fn emit(&self, state: Option<ConnState>) {
        let _ = self
            .events
            .send((self.cfg.id.clone(), HostEvent::Term(state)));
    }

    fn zellij_path(&self) -> Option<String> {
        self.cfg.zellij.clone().or_else(|| {
            let paths = self.zellij.lock().unwrap_or_else(|p| p.into_inner());
            paths.get(&self.cfg.id).cloned()
        })
    }

    /// A command that does not need the connection.
    fn local(&mut self, cmd: TermCmd) {
        match cmd {
            TermCmd::Input { id, data } => {
                if let Some(pty) = self.slots.get(&id).and_then(|s| s.pty.as_ref()) {
                    let _ = pty.send(PtyIn::Data(data));
                }
            }
            TermCmd::Resize { id, size } => {
                if let Some(slot) = self.slots.get_mut(&id) {
                    slot.spec.size = size;
                    if let Some(pty) = &slot.pty {
                        let _ = pty.send(PtyIn::Resize(size));
                    }
                }
            }
            TermCmd::Close { id } => {
                if let Some(pty) = self.slots.remove(&id).and_then(|s| s.pty) {
                    let _ = pty.send(PtyIn::Close);
                }
            }
            TermCmd::Open { spec, reply } => self.pending.push_back((spec, reply)),
            TermCmd::Retry | TermCmd::Stop => {}
        }
    }

    /// Run `fut` (connecting or attaching) while handling commands that do
    /// not need the connection; `None` if told to stop. `Retry` is ignored:
    /// an attempt is under way.
    async fn busy<F: Future>(&mut self, fut: F) -> Option<F::Output> {
        tokio::pin!(fut);
        loop {
            tokio::select! {
                out = &mut fut => return Some(out),
                cmd = self.cmds.recv() => match cmd {
                    None | Some(TermCmd::Stop) => return None,
                    Some(cmd) => self.local(cmd),
                },
            }
        }
    }

    /// Attach `spec` over the current connection and start forwarding its
    /// output; `None` if told to stop meanwhile.
    async fn attach(
        &mut self,
        id: u64,
        generation: u64,
        spec: &AttachSpec,
    ) -> Option<Result<UnboundedSender<PtyIn>, OpenError>> {
        let zellij = self.zellij_path();
        let mut t = match std::mem::replace(&mut self.link, Link::Busy) {
            Link::Up(t) => t,
            other => {
                self.link = other;
                return Some(Err(OpenError::Retry("terminal connection is down".into())));
            }
        };
        let spec = spec.clone();
        let (t, r) = self
            .busy(async move {
                let r = t.attach(zellij.as_deref(), &spec).await;
                (t, r)
            })
            .await?;
        self.link = Link::Up(t);
        let PtyIo { input, mut output } = match r {
            Ok(io) => io,
            Err(e) => return Some(Err(e)),
        };
        let tx = self.out_tx.clone();
        tokio::spawn(async move {
            while let Some(o) = output.recv().await {
                if tx.send((id, generation, Some(o))).is_err() {
                    return;
                }
            }
            let _ = tx.send((id, generation, None));
        });
        Some(Ok(input))
    }

    /// Open the connection; `None` if told to stop meanwhile.
    async fn connect(&mut self) -> Option<Result<(), OpenError>> {
        let (connector, cfg) = (self.connector.clone(), self.cfg.clone());
        let r = self
            .busy(async move { connector.open_terminals(&cfg).await })
            .await?;
        Some(r.map(|t| {
            self.link = Link::Up(t);
            self.up_since = Some(Instant::now());
            self.emit(Some(ConnState::Up));
        }))
    }

    /// Drop the connection when the last terminal is gone.
    fn idle_if_empty(&mut self) {
        if self.slots.is_empty() && !matches!(self.link, Link::Idle | Link::Busy) {
            self.link = Link::Idle;
            self.up_since = None;
            self.backoff.reset();
            self.emit(None);
        }
    }

    /// `None` if told to stop meanwhile.
    async fn open(&mut self, spec: AttachSpec) -> Option<OpenResult> {
        match self.link {
            Link::Down { .. } => {
                // Detached terminals exist: reconnect and reattach them all,
                // or they would stay detached once the link is up again.
                if !self.reconnect().await {
                    return None;
                }
                if let Link::Down { error, .. } = &self.link {
                    return Some(Err(TermError::Open(error.clone())));
                }
            }
            Link::Idle => match self.connect().await? {
                Ok(()) => {}
                Err(e) => {
                    self.link = Link::Idle;
                    self.emit(None);
                    return Some(Err(TermError::Open(e)));
                }
            },
            Link::Up(_) | Link::Busy => {}
        }
        let id = self.next_id;
        self.next_id += 1;
        match self.attach(id, 0, &spec).await? {
            Ok(pty) => {
                let (tx, rx) = mpsc::unbounded_channel();
                let _ = tx.send(TermEvent::Attached);
                self.slots.insert(
                    id,
                    Slot {
                        spec,
                        events: tx,
                        pty: Some(pty),
                        generation: 0,
                        attached_at: Instant::now(),
                        quick_ends: 0,
                    },
                );
                Some(Ok((id, rx)))
            }
            Err(e) => {
                self.idle_if_empty();
                Some(Err(TermError::Open(e)))
            }
        }
    }

    /// The connection is gone: detach every terminal and schedule a
    /// reconnect. The backoff starts over only if the connection had
    /// stayed up for `STABLE_AFTER`.
    fn lost(&mut self, reason: String) {
        for slot in self.slots.values_mut() {
            slot.pty = None;
            let _ = slot.events.send(TermEvent::Detached {
                reason: reason.clone(),
            });
        }
        if self
            .up_since
            .take()
            .is_some_and(|t| t.elapsed() >= STABLE_AFTER)
        {
            self.backoff.reset();
        }
        let retry_in = self.backoff.next_delay();
        self.link = Link::Down {
            at: Some(Instant::now() + retry_in),
            error: OpenError::Retry(reason.clone()),
        };
        self.emit(Some(ConnState::Retrying { retry_in, reason }));
    }

    /// Reconnect and attach every terminal again; false if told to stop.
    async fn reconnect(&mut self) -> bool {
        match self.connect().await {
            None => return false,
            Some(Ok(())) => {}
            Some(Err(OpenError::Retry(reason))) => {
                let retry_in = self.backoff.next_delay();
                self.link = Link::Down {
                    at: Some(Instant::now() + retry_in),
                    error: OpenError::Retry(reason.clone()),
                };
                self.emit(Some(ConnState::Retrying { retry_in, reason }));
                return true;
            }
            Some(Err(OpenError::NeedsUser(p))) => {
                self.link = Link::Down {
                    at: None,
                    error: OpenError::NeedsUser(p.clone()),
                };
                self.emit(Some(ConnState::NeedsUser(p)));
                return true;
            }
        }
        let ids: Vec<u64> = self.slots.keys().copied().collect();
        for id in ids {
            let Some(slot) = self.slots.get(&id) else {
                continue;
            };
            let (spec, generation) = (slot.spec.clone(), slot.generation + 1);
            match self.attach(id, generation, &spec).await {
                None => return false,
                Some(Ok(pty)) => match self.slots.get_mut(&id) {
                    Some(slot) => {
                        slot.pty = Some(pty);
                        slot.generation = generation;
                        slot.attached_at = Instant::now();
                        let _ = slot.events.send(TermEvent::Attached);
                    }
                    // Closed while attaching.
                    None => {
                        let _ = pty.send(PtyIn::Close);
                    }
                },
                Some(Err(e)) => {
                    self.lost(reason_of(e));
                    return true;
                }
            }
        }
        self.idle_if_empty();
        true
    }

    /// One terminal's channel closed without an exit status while the
    /// connection is up: attach that terminal again, unless its channel
    /// keeps closing right after attaching. False if told to stop.
    async fn channel_ended(&mut self, id: u64) -> bool {
        let Some(slot) = self.slots.get_mut(&id) else {
            return true;
        };
        slot.pty = None;
        slot.quick_ends = if slot.attached_at.elapsed() < STABLE_AFTER {
            slot.quick_ends + 1
        } else {
            0
        };
        if slot.quick_ends >= MAX_QUICK_ENDS {
            let _ = slot.events.send(TermEvent::Exited(None));
            self.slots.remove(&id);
            self.idle_if_empty();
            return true;
        }
        let _ = slot.events.send(TermEvent::Detached {
            reason: "terminal channel closed".into(),
        });
        let (spec, generation) = (slot.spec.clone(), slot.generation + 1);
        match self.attach(id, generation, &spec).await {
            None => false,
            Some(Ok(pty)) => {
                match self.slots.get_mut(&id) {
                    Some(slot) => {
                        slot.pty = Some(pty);
                        slot.generation = generation;
                        slot.attached_at = Instant::now();
                        let _ = slot.events.send(TermEvent::Attached);
                    }
                    None => {
                        let _ = pty.send(PtyIn::Close);
                    }
                }
                self.idle_if_empty();
                true
            }
            Some(Err(e)) => {
                self.lost(reason_of(e));
                true
            }
        }
    }

    /// False if told to stop.
    async fn output(&mut self, (id, generation, out): Tagged) -> bool {
        let Some(slot) = self.slots.get(&id) else {
            return true;
        };
        if slot.generation != generation || slot.pty.is_none() {
            return true;
        }
        match out {
            Some(PtyOut::Data(d)) => {
                let _ = slot.events.send(TermEvent::Output(d));
            }
            Some(PtyOut::Exit(status)) => {
                let _ = slot.events.send(TermEvent::Exited(status));
                self.slots.remove(&id);
                self.idle_if_empty();
            }
            None => {
                if matches!(&self.link, Link::Up(t) if t.alive()) {
                    return self.channel_ended(id).await;
                }
                self.lost("connection lost".into());
            }
        }
        true
    }

    /// Handle one command; false when the task should end.
    async fn command(&mut self, cmd: Option<TermCmd>) -> bool {
        match cmd {
            None | Some(TermCmd::Stop) => false,
            Some(TermCmd::Open { spec, reply }) => match self.open(spec).await {
                Some(r) => {
                    let _ = reply.send(r);
                    true
                }
                None => false,
            },
            Some(TermCmd::Retry) => {
                if matches!(self.link, Link::Down { .. }) {
                    return self.reconnect().await;
                }
                true
            }
            Some(cmd) => {
                self.local(cmd);
                self.idle_if_empty();
                true
            }
        }
    }

    /// Close every PTY (the task is ending).
    fn close_all(&self) {
        for slot in self.slots.values() {
            if let Some(pty) = &slot.pty {
                let _ = pty.send(PtyIn::Close);
            }
        }
    }
}

/// Run the terminals of `cfg` until `TermCmd::Stop` or until the command
/// channel closes. Connection states go to `events` as `HostEvent::Term`.
pub async fn run_terms<C: Connector>(
    cfg: HostConfig,
    connector: Arc<C>,
    zellij: ZellijPaths,
    events: UnboundedSender<(String, HostEvent)>,
    cmds: UnboundedReceiver<TermCmd>,
) {
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let mut t = Terms {
        cfg,
        connector,
        zellij,
        events,
        cmds,
        link: Link::Idle,
        slots: HashMap::new(),
        pending: VecDeque::new(),
        next_id: 1,
        backoff: Backoff::default(),
        up_since: None,
        out_tx,
    };
    loop {
        let go_on = if let Some((spec, reply)) = t.pending.pop_front() {
            match t.open(spec).await {
                Some(r) => {
                    let _ = reply.send(r);
                    true
                }
                None => false,
            }
        } else {
            let retry_at = match t.link {
                Link::Down { at: Some(at), .. } if !t.slots.is_empty() => Some(at),
                _ => None,
            };
            let sleep = tokio::time::sleep_until(retry_at.unwrap_or_else(Instant::now));
            tokio::select! {
                cmd = t.cmds.recv() => t.command(cmd).await,
                Some(out) = out_rx.recv() => t.output(out).await,
                _ = sleep, if retry_at.is_some() => t.reconnect().await,
            }
        };
        if !go_on {
            t.close_all();
            return;
        }
    }
}
```

- [ ] **Step 4: `crates/mai-core/src/terminals.rs`（完整内容，新增 `alive`）**

```rust
//! The real `TermTransport`s: an SSH session dedicated to terminals (a
//! PTY channel per terminal), or local PTYs for this machine.

use std::ffi::OsString;

use crate::deploy::Remote;
use crate::host::{OpenError, Problem};
use crate::pty::{PtyIo, spawn_local};
use crate::ssh::auth::Prompter;
use crate::ssh::client::SshSession;
use crate::term::{AttachSpec, LOCALE_ENV, TermTransport, attach_argv, remote_attach_command};

/// Terminal connection of one host.
pub enum SystemTerminals<P: Prompter> {
    /// Its own SSH session (the probe has another one), and what `detect`
    /// learned about the host's shell.
    Ssh {
        session: SshSession<P>,
        remote: Remote,
    },
    /// This machine: each terminal is a local PTY.
    Local,
}

/// Environment for a local `zellij attach`: a GUI app may have neither a
/// terminal type nor a UTF-8 locale in its environment.
pub fn local_env() -> Vec<(&'static str, &'static str)> {
    let mut env = vec![("TERM", "xterm-256color")];
    if !cfg!(windows) {
        env.extend(LOCALE_ENV);
    }
    env
}

impl<P: Prompter> TermTransport for SystemTerminals<P> {
    async fn attach(
        &mut self,
        zellij: Option<&str>,
        spec: &AttachSpec,
    ) -> Result<PtyIo, OpenError> {
        match self {
            Self::Ssh { session, remote } => {
                let (name, create) = (spec.session.as_str(), spec.create);
                session
                    .open_pty(spec.size, &LOCALE_ENV, |accepted| {
                        remote_attach_command(remote, zellij, name, create, accepted)
                    })
                    .await
                    .map_err(crate::connect::ssh_open_error)
            }
            Self::Local => {
                let program = OsString::from(zellij.unwrap_or("zellij"));
                let argv = attach_argv(&spec.session, spec.create);
                spawn_local(&program, &argv, &local_env(), spec.size).map_err(|e| {
                    OpenError::NeedsUser(Problem::Config(format!(
                        "cannot start {}: {e}",
                        program.to_string_lossy()
                    )))
                })
            }
        }
    }

    fn alive(&self) -> bool {
        match self {
            Self::Ssh { session, .. } => !session.is_closed(),
            Self::Local => true,
        }
    }
}
```

- [ ] **Step 5: `crates/mai-core/src/ssh/client.rs`（完整内容，新增 `is_closed`）**

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
                    let mut signer = FileSigner {
                        path,
                        prompter,
                        secrets,
                        notes,
                        declined: false,
                    };
                    let r = match h
                        .authenticate_publickey_with(user, public.clone(), hash, &mut signer)
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return Err(SshError::Connect(format!("{e:?}"))),
                    };
                    // A declined passphrase leaves the key to the agent.
                    if !signer.declined {
                        tried.push(public);
                    }
                    r
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

    /// Whether the connection is gone (the session task has ended).
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
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

- [ ] **Step 6: `crates/mai-core/src/manager.rs`（完整内容，先检查 session 名）**

```rust
//! Runs two tasks per host (probe connection and terminals) plus one
//! monitor task, and gives the app a small handle to add, remove and
//! command hosts and to open terminals. Every change comes back as an
//! `Update` on the receiver returned by `HostManager::start`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mai_protocol::AppMsg;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::host::{Connector, HostCommand, HostConfig, HostEvent, run_host};
use crate::monitor::{Monitor, Update};
use crate::pty::TermSize;
use crate::term::{
    AttachSpec, TermCmd, TermError, TermEvent, ZellijPaths, check_session, run_terms,
};
use crate::tracker::{AgentKey, TrackerConfig};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

enum Ctl {
    Added(String),
    Removed(String),
    Acknowledge(AgentKey),
}

/// The two tasks of one host.
struct HostTasks {
    probe: UnboundedSender<HostCommand>,
    term: UnboundedSender<TermCmd>,
}

/// Handle to the running hosts. Dropping it stops every host task and,
/// once they have ended, the monitor task (the update stream then ends).
pub struct HostManager<C> {
    connector: Arc<C>,
    hosts: HashMap<String, HostTasks>,
    events: UnboundedSender<(String, HostEvent)>,
    ctl: UnboundedSender<Ctl>,
    zellij: ZellijPaths,
}

/// An open terminal: `zellij attach` of one session. Dropping it detaches.
pub struct Terminal {
    pub host: String,
    pub id: u64,
    events: UnboundedReceiver<TermEvent>,
    cmds: UnboundedSender<TermCmd>,
}

impl Terminal {
    /// Keyboard input (bytes as the terminal emulator produces them).
    pub fn write(&self, data: Vec<u8>) {
        let _ = self.cmds.send(TermCmd::Input { id: self.id, data });
    }

    pub fn resize(&self, size: TermSize) {
        let _ = self.cmds.send(TermCmd::Resize { id: self.id, size });
    }

    /// Next event; `None` once the terminal is finished (after
    /// `TermEvent::Exited`, or when its host was removed).
    pub async fn recv(&mut self) -> Option<TermEvent> {
        self.events.recv().await
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.cmds.send(TermCmd::Close { id: self.id });
    }
}

async fn run_monitor(
    mut monitor: Monitor,
    mut events: UnboundedReceiver<(String, HostEvent)>,
    mut ctl: UnboundedReceiver<Ctl>,
    updates: UnboundedSender<Update>,
    zellij: ZellijPaths,
) {
    // Events of a removed host that were still queued are dropped.
    let mut removed: HashSet<String> = HashSet::new();
    let mut ctl_open = true;
    loop {
        // Biased: a control message sent before a host event (e.g. a
        // host re-added before its task starts) is seen first.
        let out = tokio::select! {
            biased;
            c = ctl.recv(), if ctl_open => match c {
                None => {
                    ctl_open = false;
                    continue;
                }
                Some(Ctl::Added(id)) => {
                    removed.remove(&id);
                    continue;
                }
                Some(Ctl::Removed(id)) => {
                    monitor.remove_host(&id);
                    zellij.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
                    removed.insert(id);
                    continue;
                }
                Some(Ctl::Acknowledge(key)) => monitor.acknowledge(&key).into_iter().collect(),
            },
            ev = events.recv() => match ev {
                None => return,
                Some((id, _)) if removed.contains(&id) => continue,
                Some((id, ev)) => {
                    // Terminals attach with the zellij the probe found.
                    if let HostEvent::Hello(info) = &ev {
                        let mut paths = zellij.lock().unwrap_or_else(|p| p.into_inner());
                        match &info.zellij_path {
                            Some(p) => paths.insert(id.clone(), p.clone()),
                            None => paths.remove(&id),
                        };
                    }
                    monitor.apply(&id, ev, now_ms())
                }
            },
        };
        for u in out {
            if updates.send(u).is_err() {
                return;
            }
        }
    }
}

impl<C: Connector> HostManager<C> {
    /// Start the monitor task. Must be called inside a tokio runtime.
    pub fn start(connector: Arc<C>, cfg: TrackerConfig) -> (Self, UnboundedReceiver<Update>) {
        let (events, events_rx) = mpsc::unbounded_channel();
        let (ctl, ctl_rx) = mpsc::unbounded_channel();
        let (updates, updates_rx) = mpsc::unbounded_channel();
        let zellij = ZellijPaths::default();
        tokio::spawn(run_monitor(
            Monitor::new(cfg),
            events_rx,
            ctl_rx,
            updates,
            zellij.clone(),
        ));
        let manager = Self {
            connector,
            hosts: HashMap::new(),
            events,
            ctl,
            zellij,
        };
        (manager, updates_rx)
    }

    /// Start monitoring `cfg`. False if a host with this id exists.
    pub fn add_host(&mut self, cfg: HostConfig) -> bool {
        if self.hosts.contains_key(&cfg.id) {
            return false;
        }
        let (probe, probe_rx) = mpsc::unbounded_channel();
        let (term, term_rx) = mpsc::unbounded_channel();
        let _ = self.ctl.send(Ctl::Added(cfg.id.clone()));
        self.hosts.insert(cfg.id.clone(), HostTasks { probe, term });
        tokio::spawn(run_terms(
            cfg.clone(),
            self.connector.clone(),
            self.zellij.clone(),
            self.events.clone(),
            term_rx,
        ));
        tokio::spawn(run_host(
            cfg,
            self.connector.clone(),
            self.events.clone(),
            probe_rx,
        ));
        true
    }

    /// Stop monitoring host `id`, close its terminals and forget its state.
    pub fn remove_host(&mut self, id: &str) -> bool {
        let Some(tasks) = self.hosts.remove(id) else {
            return false;
        };
        let _ = tasks.probe.send(HostCommand::Stop);
        let _ = tasks.term.send(TermCmd::Stop);
        let _ = self.ctl.send(Ctl::Removed(id.to_owned()));
        true
    }

    /// Reconnect host `id` now (after a failure that needs the user, or
    /// to skip a backoff wait); applies to both connections.
    pub fn retry(&self, id: &str) -> bool {
        let Some(tasks) = self.hosts.get(id) else {
            return false;
        };
        let _ = tasks.term.send(TermCmd::Retry);
        tasks.probe.send(HostCommand::Retry).is_ok()
    }

    /// Send `msg` to host `id`'s probe. If the probe is down, an
    /// `Update::Dropped` reports it.
    pub fn send(&self, id: &str, msg: AppMsg) -> bool {
        self.hosts
            .get(id)
            .is_some_and(|t| t.probe.send(HostCommand::Send(msg)).is_ok())
    }

    /// The user has seen this agent's alert.
    pub fn acknowledge(&self, key: AgentKey) {
        let _ = self.ctl.send(Ctl::Acknowledge(key));
    }

    /// Attach a terminal to zellij session `session` on host `host`
    /// (creating the session first if `create`). The first terminal of a
    /// host opens its terminal connection; this may prompt (host key,
    /// password) like the probe connection.
    pub async fn open_terminal(
        &self,
        host: &str,
        session: &str,
        create: bool,
        size: TermSize,
    ) -> Result<Terminal, TermError> {
        check_session(session)?;
        let tasks = self.hosts.get(host).ok_or(TermError::NoHost)?;
        let (reply, answer) = oneshot::channel();
        let spec = AttachSpec {
            session: session.to_owned(),
            create,
            size,
        };
        tasks
            .term
            .send(TermCmd::Open { spec, reply })
            .map_err(|_| TermError::Stopped)?;
        let (id, events) = answer.await.map_err(|_| TermError::Stopped)??;
        Ok(Terminal {
            host: host.to_owned(),
            id,
            events,
            cmds: tasks.term.clone(),
        })
    }

    pub fn host_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.hosts.keys().cloned().collect();
        ids.sort();
        ids
    }
}

impl<C> Drop for HostManager<C> {
    /// Stops every host's tasks. Open terminals hold a sender of their
    /// terminal task, so closing the channels alone would not end it.
    fn drop(&mut self) {
        for tasks in self.hosts.values() {
            let _ = tasks.probe.send(HostCommand::Stop);
            let _ = tasks.term.send(TermCmd::Stop);
        }
    }
}
```

- [ ] **Step 7: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: `term` 17 个、`hosts` 18 个测试通过，全部 262 个；clippy 无警告。

- [ ] **Step 8: 提交**

```bash
git add crates/mai-core
git commit -m "fix(core): terminals keep handling commands while connecting; tell a closed channel from a lost connection

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: 主机以同一 id 重新加入（B29）

**Files:**

- Modify: `crates/mai-core/src/manager.rs`（含 `#[cfg(test)]` 单元测试）

**Interfaces:**

- 内部：`Ctl::Added(String, u64)`；监视任务的事件通道为
  `(String, u64, HostEvent)`；`run_host`、`run_terms` 的签名不变（仍发
  `(String, HostEvent)`，由每台主机一个的转发任务加上代数）。

- [ ] **Step 1: 写 `crates/mai-core/src/manager.rs`（完整内容，末尾的单元测试
  `events_of_an_earlier_incarnation_are_dropped` 即本任务的测试）**

```rust
//! Runs two tasks per host (probe connection and terminals) plus one
//! monitor task, and gives the app a small handle to add, remove and
//! command hosts and to open terminals. Every change comes back as an
//! `Update` on the receiver returned by `HostManager::start`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mai_protocol::AppMsg;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::host::{Connector, HostCommand, HostConfig, HostEvent, run_host};
use crate::monitor::{Monitor, Update};
use crate::pty::TermSize;
use crate::term::{
    AttachSpec, TermCmd, TermError, TermEvent, ZellijPaths, check_session, run_terms,
};
use crate::tracker::{AgentKey, TrackerConfig};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

enum Ctl {
    /// A host was added; its events carry this generation.
    Added(String, u64),
    Removed(String),
    Acknowledge(AgentKey),
}

/// The two tasks of one host.
struct HostTasks {
    probe: UnboundedSender<HostCommand>,
    term: UnboundedSender<TermCmd>,
}

/// Handle to the running hosts. Dropping it stops every host task and,
/// once they have ended, the monitor task (the update stream then ends).
pub struct HostManager<C> {
    connector: Arc<C>,
    hosts: HashMap<String, HostTasks>,
    events: UnboundedSender<(String, u64, HostEvent)>,
    ctl: UnboundedSender<Ctl>,
    zellij: ZellijPaths,
    /// Bumped on every `add_host`.
    generation: u64,
}

/// An open terminal: `zellij attach` of one session. Dropping it detaches.
pub struct Terminal {
    pub host: String,
    pub id: u64,
    events: UnboundedReceiver<TermEvent>,
    cmds: UnboundedSender<TermCmd>,
}

impl Terminal {
    /// Keyboard input (bytes as the terminal emulator produces them).
    pub fn write(&self, data: Vec<u8>) {
        let _ = self.cmds.send(TermCmd::Input { id: self.id, data });
    }

    pub fn resize(&self, size: TermSize) {
        let _ = self.cmds.send(TermCmd::Resize { id: self.id, size });
    }

    /// Next event; `None` once the terminal is finished (after
    /// `TermEvent::Exited`, or when its host was removed).
    pub async fn recv(&mut self) -> Option<TermEvent> {
        self.events.recv().await
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.cmds.send(TermCmd::Close { id: self.id });
    }
}

async fn run_monitor(
    mut monitor: Monitor,
    mut events: UnboundedReceiver<(String, u64, HostEvent)>,
    mut ctl: UnboundedReceiver<Ctl>,
    updates: UnboundedSender<Update>,
    zellij: ZellijPaths,
) {
    // The generation of each current host. Events of a removed host, or
    // of an earlier incarnation of a host added again with the same id
    // (B29), are dropped.
    let mut current: HashMap<String, u64> = HashMap::new();
    let mut ctl_open = true;
    loop {
        // Biased: a control message sent before a host event (e.g. a
        // host re-added before its task starts) is seen first.
        let out = tokio::select! {
            biased;
            c = ctl.recv(), if ctl_open => match c {
                None => {
                    ctl_open = false;
                    continue;
                }
                Some(Ctl::Added(id, generation)) => {
                    current.insert(id, generation);
                    continue;
                }
                Some(Ctl::Removed(id)) => {
                    monitor.remove_host(&id);
                    zellij.lock().unwrap_or_else(|p| p.into_inner()).remove(&id);
                    current.remove(&id);
                    continue;
                }
                Some(Ctl::Acknowledge(key)) => monitor.acknowledge(&key).into_iter().collect(),
            },
            ev = events.recv() => match ev {
                None => return,
                Some((id, generation, _)) if current.get(&id) != Some(&generation) => continue,
                Some((id, _, ev)) => {
                    // Terminals attach with the zellij the probe found.
                    if let HostEvent::Hello(info) = &ev {
                        let mut paths = zellij.lock().unwrap_or_else(|p| p.into_inner());
                        match &info.zellij_path {
                            Some(p) => paths.insert(id.clone(), p.clone()),
                            None => paths.remove(&id),
                        };
                    }
                    monitor.apply(&id, ev, now_ms())
                }
            },
        };
        for u in out {
            if updates.send(u).is_err() {
                return;
            }
        }
    }
}

impl<C: Connector> HostManager<C> {
    /// Start the monitor task. Must be called inside a tokio runtime.
    pub fn start(connector: Arc<C>, cfg: TrackerConfig) -> (Self, UnboundedReceiver<Update>) {
        let (events, events_rx) = mpsc::unbounded_channel();
        let (ctl, ctl_rx) = mpsc::unbounded_channel();
        let (updates, updates_rx) = mpsc::unbounded_channel();
        let zellij = ZellijPaths::default();
        tokio::spawn(run_monitor(
            Monitor::new(cfg),
            events_rx,
            ctl_rx,
            updates,
            zellij.clone(),
        ));
        let manager = Self {
            connector,
            hosts: HashMap::new(),
            events,
            ctl,
            zellij,
            generation: 0,
        };
        (manager, updates_rx)
    }

    /// Start monitoring `cfg`. False if a host with this id exists.
    pub fn add_host(&mut self, cfg: HostConfig) -> bool {
        if self.hosts.contains_key(&cfg.id) {
            return false;
        }
        self.generation += 1;
        let generation = self.generation;
        let (probe, probe_rx) = mpsc::unbounded_channel();
        let (term, term_rx) = mpsc::unbounded_channel();
        let _ = self.ctl.send(Ctl::Added(cfg.id.clone(), generation));
        self.hosts.insert(cfg.id.clone(), HostTasks { probe, term });
        // Both tasks of this host report through one channel whose events
        // are tagged with the generation on the way to the monitor.
        let (host_events, mut host_rx) = mpsc::unbounded_channel::<(String, HostEvent)>();
        let events = self.events.clone();
        tokio::spawn(async move {
            while let Some((id, ev)) = host_rx.recv().await {
                if events.send((id, generation, ev)).is_err() {
                    return;
                }
            }
        });
        tokio::spawn(run_terms(
            cfg.clone(),
            self.connector.clone(),
            self.zellij.clone(),
            host_events.clone(),
            term_rx,
        ));
        tokio::spawn(run_host(cfg, self.connector.clone(), host_events, probe_rx));
        true
    }

    /// Stop monitoring host `id`, close its terminals and forget its state.
    pub fn remove_host(&mut self, id: &str) -> bool {
        let Some(tasks) = self.hosts.remove(id) else {
            return false;
        };
        let _ = tasks.probe.send(HostCommand::Stop);
        let _ = tasks.term.send(TermCmd::Stop);
        let _ = self.ctl.send(Ctl::Removed(id.to_owned()));
        true
    }

    /// Reconnect host `id` now (after a failure that needs the user, or
    /// to skip a backoff wait); applies to both connections.
    pub fn retry(&self, id: &str) -> bool {
        let Some(tasks) = self.hosts.get(id) else {
            return false;
        };
        let _ = tasks.term.send(TermCmd::Retry);
        tasks.probe.send(HostCommand::Retry).is_ok()
    }

    /// Send `msg` to host `id`'s probe. If the probe is down, an
    /// `Update::Dropped` reports it.
    pub fn send(&self, id: &str, msg: AppMsg) -> bool {
        self.hosts
            .get(id)
            .is_some_and(|t| t.probe.send(HostCommand::Send(msg)).is_ok())
    }

    /// The user has seen this agent's alert.
    pub fn acknowledge(&self, key: AgentKey) {
        let _ = self.ctl.send(Ctl::Acknowledge(key));
    }

    /// Attach a terminal to zellij session `session` on host `host`
    /// (creating the session first if `create`). The first terminal of a
    /// host opens its terminal connection; this may prompt (host key,
    /// password) like the probe connection.
    pub async fn open_terminal(
        &self,
        host: &str,
        session: &str,
        create: bool,
        size: TermSize,
    ) -> Result<Terminal, TermError> {
        check_session(session)?;
        let tasks = self.hosts.get(host).ok_or(TermError::NoHost)?;
        let (reply, answer) = oneshot::channel();
        let spec = AttachSpec {
            session: session.to_owned(),
            create,
            size,
        };
        tasks
            .term
            .send(TermCmd::Open { spec, reply })
            .map_err(|_| TermError::Stopped)?;
        let (id, events) = answer.await.map_err(|_| TermError::Stopped)??;
        Ok(Terminal {
            host: host.to_owned(),
            id,
            events,
            cmds: tasks.term.clone(),
        })
    }

    pub fn host_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.hosts.keys().cloned().collect();
        ids.sort();
        ids
    }
}

impl<C> Drop for HostManager<C> {
    /// Stops every host's tasks. Open terminals hold a sender of their
    /// terminal task, so closing the channels alone would not end it.
    fn drop(&mut self) {
        for tasks in self.hosts.values() {
            let _ = tasks.probe.send(HostCommand::Stop);
            let _ = tasks.term.send(TermCmd::Stop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ConnState;
    use std::time::Duration;

    async fn next(rx: &mut UnboundedReceiver<Update>) -> Option<Update> {
        tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .ok()
            .flatten()
    }

    /// B29: events still queued from a removed host's tasks do not reach a
    /// host added again with the same id.
    #[tokio::test(start_paused = true)]
    async fn events_of_an_earlier_incarnation_are_dropped() {
        let (events, events_rx) = mpsc::unbounded_channel();
        let (ctl, ctl_rx) = mpsc::unbounded_channel();
        let (updates, mut updates_rx) = mpsc::unbounded_channel();
        let monitor = Monitor::new(TrackerConfig::default());
        tokio::spawn(run_monitor(
            monitor,
            events_rx,
            ctl_rx,
            updates,
            ZellijPaths::default(),
        ));
        let connecting = || HostEvent::Probe(ConnState::Connecting);

        ctl.send(Ctl::Added("h".into(), 1)).unwrap();
        events.send(("h".into(), 1, connecting())).unwrap();
        assert!(next(&mut updates_rx).await.is_some());

        // Removed and added again; the old task's event arrives late.
        ctl.send(Ctl::Removed("h".into())).unwrap();
        ctl.send(Ctl::Added("h".into(), 2)).unwrap();
        events.send(("h".into(), 1, connecting())).unwrap();
        assert!(next(&mut updates_rx).await.is_none(), "stale event dropped");
        events.send(("h".into(), 2, connecting())).unwrap();
        assert!(next(&mut updates_rx).await.is_some());

        // A removed host's events are dropped too.
        ctl.send(Ctl::Removed("h".into())).unwrap();
        events.send(("h".into(), 2, connecting())).unwrap();
        assert!(next(&mut updates_rx).await.is_none());
    }
}
```

- [ ] **Step 2: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: `mai-core` 单元测试 4 个（含新测试），全部 263 个；clippy 无警告。
（先只写测试、不改实现时编译失败：`Ctl::Added` 只有一个字段。）

- [ ] **Step 3: 提交**

```bash
git add crates/mai-core
git commit -m "fix(core): drop events of a removed host's earlier incarnation

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: zellij 路径与本机 locale（B30、B32）

**Files:**

- Modify: `crates/mai-core/src/terminals.rs`、`crates/mai-core/src/connect.rs`
- Create: `crates/mai-core/tests/terminals.rs`

**Interfaces:**

- Consumes: Task 2 的 `terminals.rs`（`alive`）。
- Produces:

  ```rust
  // terminals
  pub enum SystemTerminals<P> {
      Ssh { session: SshSession<P>, remote: Remote, found: Option<String> },
      Local,
  }
  pub const ZELLIJ_DIRS: [&str; 4];          // "/opt/homebrew/bin", "/usr/local/bin", "~/.cargo/bin", "~/.local/bin"
  pub fn find_zellij_script() -> String       // POSIX sh 脚本
  pub fn find_zellij_command() -> String      // sh -c '<script>'
  pub fn found_zellij(out: &str) -> Option<String>
  pub fn local_zellij(path_env: Option<&OsStr>, home: Option<&Path>) -> OsString
  pub const FALLBACK_LOCALE: &str;            // Linux "C.UTF-8"，其他 "en_US.UTF-8"
  pub fn local_env_with(var: impl Fn(&str) -> Option<String>) -> Vec<(&'static str, &'static str)>
  pub fn local_env() -> Vec<(&'static str, &'static str)>
  ```

  远程 attach 的 zellij：主机配置 > 探针报告 > `found` > PATH。
  `connect::open_ssh_terminals` 在 POSIX 主机上执行 `find_zellij_command()`，
  失败时 `found` 为 `None`。

- [ ] **Step 1: 写失败测试 `crates/mai-core/tests/terminals.rs`**

（`#[cfg(unix)]` 的测试在 Windows 上不编译；Windows 上本文件 3 个测试，
Linux/macOS 上 6 个。）

```rust
//! Finding zellij for terminals when no path is known (B30) and the
//! locale of local terminals (B32).

use std::collections::HashMap;

use mai_core::terminals::{
    FALLBACK_LOCALE, ZELLIJ_DIRS, find_zellij_command, find_zellij_script, found_zellij,
    local_env_with,
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
    // LC_ALL wins over LANG.
    assert_eq!(
        env_of(&[("LC_ALL", "C"), ("LANG", "en_US.UTF-8")]),
        fallback
    );
    assert_eq!(
        env_of(&[("LC_ALL", ""), ("LANG", "de_DE.UTF-8")]),
        term_only
    );
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test terminals`
Expected: 编译失败（`find_zellij_script` 等不存在）。

- [ ] **Step 3: 写 `crates/mai-core/src/terminals.rs`（完整内容）**

```rust
//! The real `TermTransport`s: an SSH session dedicated to terminals (a
//! PTY channel per terminal), or local PTYs for this machine.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::deploy::Remote;
use crate::host::{OpenError, Problem};
use crate::pty::{PtyIo, spawn_local};
use crate::ssh::auth::Prompter;
use crate::ssh::client::SshSession;
use crate::term::{AttachSpec, LOCALE_ENV, TermTransport, attach_argv, remote_attach_command};

/// Terminal connection of one host.
pub enum SystemTerminals<P: Prompter> {
    /// Its own SSH session (the probe has another one), what `detect`
    /// learned about the host's shell, and the zellij found on the host
    /// (used when neither the host config nor the probe names one).
    Ssh {
        session: SshSession<P>,
        remote: Remote,
        found: Option<String>,
    },
    /// This machine: each terminal is a local PTY.
    Local,
}

/// Directories searched for zellij after PATH, relative to `$HOME` when
/// they start with `~/` (the same list the probe uses).
pub const ZELLIJ_DIRS: [&str; 4] = [
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "~/.cargo/bin",
    "~/.local/bin",
];

/// POSIX `sh` script that prints where zellij is: PATH, then
/// `ZELLIJ_DIRS`, then the user's login shell (whose profile may extend
/// PATH). Non-interactive SSH sessions often lack the directories a
/// package manager adds (e.g. `/opt/homebrew/bin` on macOS).
pub fn find_zellij_script() -> String {
    let dirs: Vec<String> = ZELLIJ_DIRS
        .iter()
        .map(|d| match d.strip_prefix("~/") {
            Some(rest) => format!("\"$HOME/{rest}\""),
            None => (*d).to_owned(),
        })
        .collect();
    format!(
        "command -v zellij || for d in {}; do if [ -x \"$d/zellij\" ]; then echo \"$d/zellij\"; exit 0; fi; done; exec \"${{SHELL:-sh}}\" -lc \"command -v zellij\"",
        dirs.join(" ")
    )
}

/// `find_zellij_script` run by `sh`, whatever the login shell is.
pub fn find_zellij_command() -> String {
    format!("sh -c '{}'", find_zellij_script())
}

/// The path printed by `find_zellij_command`: the last line that is an
/// absolute path (a login shell's startup files may print other lines).
pub fn found_zellij(out: &str) -> Option<String> {
    out.lines()
        .map(str::trim)
        .rfind(|l| l.starts_with('/') && !l.contains(' '))
        .map(str::to_owned)
}

/// The zellij to run locally when none is known: `zellij` if it is on
/// PATH, else the first of `ZELLIJ_DIRS` (under `home`) that has it.
pub fn local_zellij(path_env: Option<&std::ffi::OsStr>, home: Option<&Path>) -> OsString {
    let exe = if cfg!(windows) {
        "zellij.exe"
    } else {
        "zellij"
    };
    let on_path = path_env
        .into_iter()
        .flat_map(std::env::split_paths)
        .any(|d| d.join(exe).is_file());
    if on_path || cfg!(windows) {
        return OsString::from("zellij");
    }
    ZELLIJ_DIRS
        .iter()
        .filter_map(|d| match d.strip_prefix("~/") {
            Some(rest) => home.map(|h| h.join(rest)),
            None => Some(PathBuf::from(d)),
        })
        .map(|d| d.join(exe))
        .find(|p| p.is_file())
        .map_or_else(|| OsString::from("zellij"), PathBuf::into_os_string)
}

/// UTF-8 locale used when the environment has none: `C.UTF-8` exists on
/// every current Linux; macOS has `en_US.UTF-8`.
pub const FALLBACK_LOCALE: &str = if cfg!(target_os = "linux") {
    "C.UTF-8"
} else {
    "en_US.UTF-8"
};

/// Whether the locale in effect (`LC_ALL`, else `LC_CTYPE`, else `LANG`)
/// is UTF-8.
fn has_utf8_locale(var: &impl Fn(&str) -> Option<String>) -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .filter_map(|k| var(k).filter(|v| !v.is_empty()))
        .next()
        .is_some_and(|v| {
            let v = v.to_ascii_lowercase();
            v.contains("utf-8") || v.contains("utf8")
        })
}

/// Environment for a local `zellij attach`, given the app's environment
/// (`var`): a terminal type, and a UTF-8 locale only when the environment
/// has none (a GUI app may have neither). A user's own UTF-8 locale (e.g.
/// `zh_CN.UTF-8`) is kept.
pub fn local_env_with(var: impl Fn(&str) -> Option<String>) -> Vec<(&'static str, &'static str)> {
    let mut env = vec![("TERM", "xterm-256color")];
    if !cfg!(windows) && !has_utf8_locale(&var) {
        env.extend([("LANG", FALLBACK_LOCALE), ("LC_CTYPE", FALLBACK_LOCALE)]);
    }
    env
}

/// `local_env_with` for this process's environment.
pub fn local_env() -> Vec<(&'static str, &'static str)> {
    local_env_with(|k| std::env::var(k).ok())
}

impl<P: Prompter> TermTransport for SystemTerminals<P> {
    async fn attach(
        &mut self,
        zellij: Option<&str>,
        spec: &AttachSpec,
    ) -> Result<PtyIo, OpenError> {
        match self {
            Self::Ssh {
                session,
                remote,
                found,
            } => {
                let zellij = zellij.or(found.as_deref());
                let (name, create) = (spec.session.as_str(), spec.create);
                session
                    .open_pty(spec.size, &LOCALE_ENV, |accepted| {
                        remote_attach_command(remote, zellij, name, create, accepted)
                    })
                    .await
                    .map_err(crate::connect::ssh_open_error)
            }
            Self::Local => {
                let program = match zellij {
                    Some(z) => OsString::from(z),
                    None => {
                        let home = std::env::var_os("HOME").map(PathBuf::from);
                        local_zellij(std::env::var_os("PATH").as_deref(), home.as_deref())
                    }
                };
                let argv = attach_argv(&spec.session, spec.create);
                spawn_local(&program, &argv, &local_env(), spec.size).map_err(|e| {
                    OpenError::NeedsUser(Problem::Config(format!(
                        "cannot start {}: {e}",
                        program.to_string_lossy()
                    )))
                })
            }
        }
    }

    fn alive(&self) -> bool {
        match self {
            Self::Ssh { session, .. } => !session.is_closed(),
            Self::Local => true,
        }
    }
}
```

- [ ] **Step 4: `crates/mai-core/src/connect.rs`（完整内容）**

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
use crate::terminals::{SystemTerminals, find_zellij_command, found_zellij};

/// Locale requested for `serve` so zellij output is UTF-8.
const LOCALE: &str = "en_US.UTF-8";

/// Append ssh config warnings (an ignored `Match` block may have set the
/// user or key) to an authentication failure. Other errors are unchanged.
pub fn with_config_warnings(err: OpenError, warnings: &[String]) -> OpenError {
    match err {
        OpenError::NeedsUser(Problem::Auth(m)) if !warnings.is_empty() => {
            OpenError::NeedsUser(Problem::Auth(format!("{m} ({})", warnings.join("; "))))
        }
        other => other,
    }
}

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
            .map_err(|e| with_config_warnings(ssh_open_error(e), &notes))?;
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
        // Used when neither the host config nor the probe names a zellij
        // (the probe may be down); a failed search just leaves PATH.
        let found = if remote.shell == Shell::Posix {
            match session.exec(&find_zellij_command()).await {
                Ok(out) => found_zellij(&out.stdout_str()),
                Err(_) => None,
            }
        } else {
            None
        };
        Ok(SystemTerminals::Ssh {
            session,
            remote,
            found,
        })
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

- [ ] **Step 5: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: `terminals` 3 个（Windows）测试通过，全部 266 个；clippy 无警告。

- [ ] **Step 6: 提交**

```bash
git add crates/mai-core
git commit -m "fix(core): find zellij for terminals without the probe; keep the user's UTF-8 locale

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: agent、口令键迁移与签名错误（B38、B39、B40）

**Files:**

- Modify: `crates/mai-core/src/ssh/client.rs`、`crates/mai-core/src/ssh/signer.rs`、
  `crates/mai-core/src/ssh/auth.rs`、`crates/mai-core/examples/common/mod.rs`
- Test: `crates/mai-core/tests/ssh_session.rs`、`crates/mai-core/tests/ssh_pty.rs`

**Interfaces:**

- Consumes: Task 2 的 `client.rs`。
- Produces:

  ```rust
  // ssh::client
  pub struct ConnectOptions { .., pub use_agent: bool }   // 新字段
  // ssh::auth
  pub fn legacy_passphrase_key(key_file: &Path) -> String  // "passphrase:<写法>"
  // ssh::signer
  impl Display for SignError   // "the SSH session closed during public key authentication"
  impl std::error::Error for SignError
  ```

  `load_key`：当前键下没有条目而旧键下有时，能解密则移到当前键并删除旧条目，
  不能解密也删除旧条目（之后照常询问）；钥匙串失败写入 notes。

- [ ] **Step 1: 写失败测试**

`crates/mai-core/tests/ssh_session.rs`（完整内容）：

```rust
//! SSH client tests against an in-process russh server (no network, no
//! external sshd). Keys are generated per test run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mai_core::ssh::auth::{
    KbdPrompt, Prompter, Secret, SecretStore, legacy_passphrase_key, passphrase_key, password_key,
};
use mai_core::ssh::client::{ConnectOptions, SshError, connect};
use mai_core::ssh::config::HostSpec;
use mai_core::ssh::signer::{FileSigner, SignError, load_key};
use russh::keys::agent::AgentIdentity;
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
        use_agent: false,
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
        use_agent: false,
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

async fn sign_with(passphrase: Option<Secret>) -> (Vec<u8>, bool) {
    let client = random_key();
    let dir = tempfile::tempdir().unwrap();
    let key_file = dir.path().join("id_enc");
    client
        .encrypt(&mut SysRng, "pw")
        .unwrap()
        .write_openssh_file(&key_file, LineEnding::LF)
        .unwrap();
    let p = Scripted {
        passphrase,
        ..Default::default()
    };
    let store = MemStore::default();
    let mut notes = Vec::new();
    let mut signer = FileSigner {
        path: &key_file,
        prompter: &p,
        secrets: &store,
        notes: &mut notes,
        declined: false,
    };
    let id = AgentIdentity::PublicKey {
        key: client.public_key().clone(),
        comment: String::new(),
    };
    let out = russh::Signer::auth_sign(&mut signer, &id, None, b"data".to_vec())
        .await
        .unwrap();
    (out, signer.declined)
}

#[tokio::test]
async fn file_signer_marks_a_refused_passphrase_as_declined() {
    let (out, declined) = sign_with(None).await;
    assert!(out.len() > b"data".len());
    assert!(declined);
}

#[tokio::test]
async fn file_signer_is_not_declined_with_the_right_passphrase() {
    let (out, declined) = sign_with(secret("pw", false)).await;
    assert!(out.len() > b"data".len());
    assert!(!declined);
}

/// An encrypted key file reached through a `sub/..` detour, so the path as
/// written differs from its canonical form.
fn detoured_encrypted_key(dir: &Path) -> PathBuf {
    std::fs::create_dir(dir.join("sub")).unwrap();
    random_key()
        .encrypt(&mut SysRng, "pw")
        .unwrap()
        .write_openssh_file(dir.join("id_enc"), LineEnding::LF)
        .unwrap();
    dir.join("sub").join("..").join("id_enc")
}

/// B39: a passphrase remembered before keys were canonicalized is used
/// without asking, then moved to the current key.
#[tokio::test]
async fn passphrase_under_the_old_key_is_moved_to_the_new_one() {
    let dir = tempfile::tempdir().unwrap();
    let key_file = detoured_encrypted_key(dir.path());
    let (old, new) = (legacy_passphrase_key(&key_file), passphrase_key(&key_file));
    assert_ne!(old, new);
    let store = MemStore::default();
    store.set(&old, "pw").unwrap();
    let p = Scripted::default();
    let mut notes = Vec::new();
    assert!(load_key(&key_file, &p, &store, &mut notes).await.is_some());
    assert!(p.calls().is_empty(), "not asked: {:?}", p.calls());
    assert_eq!(store.get(&new).as_deref(), Some("pw"));
    assert_eq!(store.get(&old), None, "old entry removed");
    assert!(notes.is_empty(), "{notes:?}");
}

#[tokio::test]
async fn wrong_passphrase_under_the_old_key_is_removed_and_the_user_asked() {
    let dir = tempfile::tempdir().unwrap();
    let key_file = detoured_encrypted_key(dir.path());
    let old = legacy_passphrase_key(&key_file);
    let store = MemStore::default();
    store.set(&old, "stale").unwrap();
    let p = Scripted {
        passphrase: secret("pw", false),
        ..Default::default()
    };
    let mut notes = Vec::new();
    assert!(load_key(&key_file, &p, &store, &mut notes).await.is_some());
    assert_eq!(p.calls(), vec!["passphrase"]);
    assert_eq!(store.get(&old), None);
}

/// B40: the signer's error reads as a sentence, not a debug dump.
#[test]
fn sign_error_explains_itself() {
    let msg = SignError.to_string();
    assert!(msg.contains("session closed"), "{msg}");
}
```

`crates/mai-core/tests/ssh_pty.rs`（最终完整内容，`ConnectOptions` 加
`use_agent: false`）：

```rust
//! `SshSession::open_pty` against an in-process russh server that records
//! channel requests, echoes input, reports resizes and can exit or drop.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mai_core::pty::{PtyIn, PtyIo, PtyOut, TermSize};
use mai_core::ssh::auth::{KbdPrompt, Prompter, Secret, SecretStore};
use mai_core::ssh::client::{ConnectOptions, SshSession, connect};
use mai_core::ssh::config::HostSpec;
use russh::keys::{Algorithm, PrivateKey};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId, MethodKind, MethodSet};
use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;
use tokio::time::timeout;

type Log = Arc<Mutex<Vec<String>>>;

/// How the server answers `env` requests.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Env {
    Accept,
    Refuse,
    /// Never answers.
    Silent,
}

#[derive(Clone)]
struct PtyServer {
    env: Env,
    refuse_pty: bool,
    log: Log,
}

impl PtyServer {
    fn note(&self, s: String) {
        self.log.lock().unwrap().push(s);
    }
}

impl server::Handler for PtyServer {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
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

    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        _modes: &[(russh::Pty, u32)],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.note(format!("pty {term} {cols}x{rows}"));
        if self.refuse_pty {
            session.channel_failure(channel)
        } else {
            session.channel_success(channel)
        }
    }

    async fn env_request(
        &mut self,
        channel: ChannelId,
        name: &str,
        value: &str,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        match self.env {
            Env::Accept => {
                self.note(format!("env {name}={value}"));
                session.channel_success(channel)
            }
            Env::Refuse => session.channel_failure(channel),
            Env::Silent => Ok(()),
        }
    }

    async fn channel_close(
        &mut self,
        _channel: ChannelId,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.note("close".into());
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        self.note(format!("exec {}", String::from_utf8_lossy(data)));
        if data == b"early" {
            // Output that arrives before the answer to the request.
            session.data(channel, b"early output\r\n".to_vec())?;
        }
        session.channel_success(channel)?;
        if data == b"two-streams" {
            session.data(channel, b"out-1\n".to_vec())?;
            session.extended_data(channel, 1, b"warn: first\nerror: bad rules\n".to_vec())?;
            session.data(channel, b"out-2\n".to_vec())?;
            session.exit_status_request(channel, 1)?;
            session.eof(channel)?;
            return session.close(channel);
        }
        session.data(channel, b"ready\r\n".to_vec())
    }

    /// `exit` ends with status 5; `drop` closes without a status (lost
    /// connection); anything else is echoed.
    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if data.starts_with(b"exit") {
            session.exit_status_request(channel, 5)?;
            session.eof(channel)?;
            session.close(channel)
        } else if data.starts_with(b"drop") {
            session.close(channel)
        } else {
            session.data(channel, data.to_vec())
        }
    }

    async fn window_change_request(
        &mut self,
        channel: ChannelId,
        cols: u32,
        rows: u32,
        _pw: u32,
        _ph: u32,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, format!("size={cols}x{rows}\r\n").into_bytes())
    }
}

async fn start(server: PtyServer) -> u16 {
    let key = PrivateKey::random(&mut UnwrapErr(SysRng), Algorithm::Ed25519).unwrap();
    let config = Arc::new(server::Config {
        keys: vec![key],
        methods: MethodSet::from(&[MethodKind::None][..]),
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let (config, server) = (config.clone(), server.clone());
            tokio::spawn(async move {
                let _ = server::run_stream(config, stream, server).await;
            });
        }
    });
    port
}

/// Trusts every host key; never has secrets.
struct Trusting;

impl Prompter for Trusting {
    async fn confirm_host_key(&self, _h: &str, _p: u16, _f: &str) -> bool {
        true
    }
    async fn password(&self, _u: &str, _h: &str) -> Option<Secret> {
        None
    }
    async fn passphrase(&self, _k: &Path) -> Option<Secret> {
        None
    }
    async fn keyboard_interactive(
        &self,
        _n: &str,
        _i: &str,
        _p: &[KbdPrompt],
    ) -> Option<Vec<String>> {
        None
    }
}

struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _key: &str) -> Option<String> {
        None
    }
    fn set(&self, _key: &str, _value: &str) -> Result<(), String> {
        Ok(())
    }
    fn delete(&self, _key: &str) -> Result<(), String> {
        Ok(())
    }
}

async fn session(accept_env: bool) -> (SshSession<Trusting>, Log, tempfile::TempDir) {
    let env = if accept_env { Env::Accept } else { Env::Refuse };
    session_with(env, false).await
}

async fn session_with(
    env: Env,
    refuse_pty: bool,
) -> (SshSession<Trusting>, Log, tempfile::TempDir) {
    let log = Log::default();
    let port = start(PtyServer {
        env,
        refuse_pty,
        log: log.clone(),
    })
    .await;
    let dir = tempfile::tempdir().unwrap();
    let learn_to = dir.path().join("known_hosts");
    let opts = ConnectOptions {
        known_hosts: vec![learn_to.clone()],
        learn_to,
        timeout: Duration::from_secs(10),
        use_agent: false,
    };
    let spec = HostSpec {
        alias: "pty".into(),
        host: "127.0.0.1".into(),
        port,
        user: "tester".into(),
        identity_files: vec![],
        jumps: vec![],
    };
    let s = connect(&spec, &opts, Arc::new(Trusting), &NoSecrets)
        .await
        .expect("connect");
    (s, log, dir)
}

const ENV: [(&str, &str); 2] = [("LANG", "en_US.UTF-8"), ("LC_CTYPE", "en_US.UTF-8")];

fn command(accepted: bool) -> String {
    if accepted {
        "zellij attach work".into()
    } else {
        "LANG=en_US.UTF-8 LC_CTYPE=en_US.UTF-8 zellij attach work".into()
    }
}

/// Read output until it contains `needle`.
async fn read_until(io: &mut PtyIo, needle: &str) {
    let mut seen = String::new();
    while !seen.contains(needle) {
        match timeout(Duration::from_secs(10), io.output.recv()).await {
            Ok(Some(PtyOut::Data(d))) => seen.push_str(&String::from_utf8_lossy(&d)),
            other => panic!("waiting for {needle:?}, got {other:?} after {seen:?}"),
        }
    }
}

/// The next non-data event, or `None` when output ended.
async fn end(io: &mut PtyIo) -> Option<PtyOut> {
    loop {
        match timeout(Duration::from_secs(10), io.output.recv()).await {
            Ok(Some(PtyOut::Data(_))) => continue,
            Ok(other) => return other,
            Err(_) => panic!("pty did not end"),
        }
    }
}

#[tokio::test]
async fn pty_carries_input_output_resize_and_exit() {
    let (s, log, _dir) = session(true).await;
    let size = TermSize {
        cols: 100,
        rows: 30,
    };
    let mut io = s.open_pty(size, &ENV, command).await.unwrap();
    read_until(&mut io, "ready").await;
    io.input.send(PtyIn::Data(b"hello".to_vec())).unwrap();
    read_until(&mut io, "hello").await;
    io.input
        .send(PtyIn::Resize(TermSize {
            cols: 120,
            rows: 40,
        }))
        .unwrap();
    read_until(&mut io, "size=120x40").await;
    io.input.send(PtyIn::Data(b"exit".to_vec())).unwrap();
    assert_eq!(end(&mut io).await, Some(PtyOut::Exit(Some(5))));
    assert_eq!(
        log.lock().unwrap().clone(),
        vec![
            "pty xterm-256color 100x30",
            "env LANG=en_US.UTF-8",
            "env LC_CTYPE=en_US.UTF-8",
            "exec zellij attach work",
        ]
    );
}

#[tokio::test]
async fn refused_env_falls_back_to_command_prefix() {
    let (s, log, _dir) = session(false).await;
    let size = TermSize { cols: 80, rows: 24 };
    let mut io = s.open_pty(size, &ENV, command).await.unwrap();
    read_until(&mut io, "ready").await;
    assert_eq!(
        log.lock().unwrap().last().cloned(),
        Some("exec LANG=en_US.UTF-8 LC_CTYPE=en_US.UTF-8 zellij attach work".to_owned())
    );
}

#[tokio::test]
async fn channel_closed_without_exit_status_is_a_lost_connection() {
    let (s, _log, _dir) = session(true).await;
    let size = TermSize { cols: 80, rows: 24 };
    let mut io = s.open_pty(size, &ENV, command).await.unwrap();
    read_until(&mut io, "ready").await;
    io.input.send(PtyIn::Data(b"drop".to_vec())).unwrap();
    assert_eq!(end(&mut io).await, None, "no Exit: the connection was lost");
}

/// Wait until the server has seen the channel close.
async fn closed(log: &Log) {
    timeout(Duration::from_secs(10), async {
        while !log.lock().unwrap().iter().any(|l| l == "close") {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("channel closed on the server");
}

#[tokio::test]
async fn refused_pty_closes_the_channel() {
    let (s, log, _dir) = session_with(Env::Accept, true).await;
    let size = TermSize { cols: 80, rows: 24 };
    let err = s.open_pty(size, &ENV, command).await.err().unwrap();
    assert!(err.to_string().contains("refused the pty"), "{err}");
    closed(&log).await;
}

/// A server that never answers fails the setup (instead of taking a late
/// answer for the next request's) and the channel is closed.
#[tokio::test]
async fn unanswered_request_fails_and_closes_the_channel() {
    let (s, log, _dir) = session_with(Env::Silent, false).await;
    let size = TermSize { cols: 80, rows: 24 };
    let err = s.open_pty(size, &ENV, command).await.err().unwrap();
    assert!(
        err.to_string().contains("no answer to the env request"),
        "{err}"
    );
    closed(&log).await;
    assert!(
        !log.lock().unwrap().iter().any(|l| l.starts_with("exec")),
        "no command after an unanswered request"
    );
}

#[tokio::test]
async fn output_before_the_command_is_accepted_is_kept() {
    let (s, _log, _dir) = session(true).await;
    let size = TermSize { cols: 80, rows: 24 };
    let mut io = s.open_pty(size, &ENV, |_| "early".into()).await.unwrap();
    read_until(&mut io, "early output").await;
}

#[tokio::test]
async fn exec_split_separates_stdout_from_stderr() {
    use mai_core::stderr::StderrTail;
    use tokio::io::AsyncReadExt;

    let (s, _log, _dir) = session(true).await;
    let tail = Arc::new(StderrTail::default());
    let (mut out, _stdin) = s
        .open_exec_split("two-streams", &[], tail.clone())
        .await
        .unwrap();
    let mut text = String::new();
    timeout(Duration::from_secs(10), out.read_to_string(&mut text))
        .await
        .expect("stdout ends")
        .unwrap();
    assert_eq!(text, "out-1\nout-2\n");
    assert_eq!(
        tail.summary().await.as_deref(),
        Some("warn: first | error: bad rules")
    );
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test ssh_session --test ssh_pty`
Expected: 编译失败（`use_agent`、`legacy_passphrase_key`、`SignError` 的 Display 不存在）。

- [ ] **Step 3: `crates/mai-core/src/ssh/auth.rs`（完整内容）**

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

/// The key passphrases were stored under before paths were canonicalized
/// (the path as written). Looked up once and moved to `passphrase_key`.
pub fn legacy_passphrase_key(key_file: &Path) -> String {
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

- [ ] **Step 4: `crates/mai-core/src/ssh/signer.rs`（完整内容）**

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
```

- [ ] **Step 5: `crates/mai-core/src/ssh/client.rs`（完整内容）**

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
    /// Offer the keys of the running ssh-agent (or Pageant on Windows).
    /// Tests turn this off so they never reach the developer's agent.
    pub use_agent: bool,
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
    authenticate(
        &mut handle,
        hop,
        prompter.as_ref(),
        secrets,
        opts.use_agent,
        &mut notes,
    )
    .await?;
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
    use_agent: bool,
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
                    let mut signer = FileSigner {
                        path,
                        prompter,
                        secrets,
                        notes,
                        declined: false,
                    };
                    let r = match h
                        .authenticate_publickey_with(user, public.clone(), hash, &mut signer)
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return Err(connect_err(e)),
                    };
                    // A declined passphrase leaves the key to the agent.
                    if !signer.declined {
                        tried.push(public);
                    }
                    r
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
        if use_agent && try_agent(h, user, &tried, &mut methods).await? == Step::Done {
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

    /// Whether the connection is gone (the session task has ended).
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
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

- [ ] **Step 6: `crates/mai-core/examples/common/mod.rs`（完整内容，`use_agent: true`）**

```rust
//! Shared helpers for the manual-check examples.
//!
//! Environment:
//! - `MAI_SSH_CONFIG`: ssh config file (default `~/.ssh/config`)
//! - `MAI_LEARN_TO`: where confirmed host keys go (default: a temp file);
//!   it is also checked
//! - `MAI_SKIP_USER_KNOWN_HOSTS=1`: ignore `~/.ssh/known_hosts`
//!
//! Prompts are read from the terminal; passwords, passphrases and
//! non-echo keyboard-interactive answers are read without echo.
//! Secrets are kept in memory, never in the OS keychain.

use std::collections::HashMap;
use std::future::Future;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mai_core::ssh::auth::{KbdPrompt, Prompter, Secret, SecretStore};
use mai_core::ssh::client::{ConnectOptions, SshSession, connect};
use mai_core::ssh::config::{HostSpec, parse_config, resolve};

pub struct TermPrompter;

fn ask(question: &str) -> Option<String> {
    print!("{question}");
    std::io::stdout().flush().ok()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).ok()?;
    let line = line.trim_end().to_owned();
    (!line.is_empty()).then_some(line)
}

fn ask_hidden(question: &str) -> Option<String> {
    let line = rpassword::prompt_password(question).ok()?;
    (!line.is_empty()).then_some(line)
}

impl Prompter for TermPrompter {
    fn confirm_host_key(
        &self,
        host: &str,
        port: u16,
        fingerprint: &str,
    ) -> impl Future<Output = bool> + Send {
        let q = format!("Unknown host key for {host}:{port}\n  {fingerprint}\nTrust it? [y/N] ");
        async move { ask(&q).is_some_and(|a| a.eq_ignore_ascii_case("y")) }
    }

    fn password(&self, user: &str, host: &str) -> impl Future<Output = Option<Secret>> + Send {
        let q = format!("Password for {user}@{host}: ");
        async move {
            ask_hidden(&q).map(|value| Secret {
                value,
                remember: false,
            })
        }
    }

    fn passphrase(&self, key_file: &Path) -> impl Future<Output = Option<Secret>> + Send {
        let q = format!("Passphrase for {}: ", key_file.display());
        async move {
            ask_hidden(&q).map(|value| Secret {
                value,
                remember: false,
            })
        }
    }

    fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[KbdPrompt],
    ) -> impl Future<Output = Option<Vec<String>>> + Send {
        let header = format!("{name} {instructions}");
        let asks: Vec<KbdPrompt> = prompts.to_vec();
        async move {
            println!("{header}");
            asks.iter()
                .map(|p| {
                    if p.echo {
                        ask(&p.text)
                    } else {
                        ask_hidden(&p.text)
                    }
                })
                .collect()
        }
    }
}

#[derive(Default)]
pub struct MemoryStore(Mutex<HashMap<String, String>>);

impl SecretStore for MemoryStore {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().ok()?.get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|e| e.to_string())?
            .insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.0.lock().map_err(|e| e.to_string())?.remove(key);
        Ok(())
    }
}

pub fn home() -> PathBuf {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map_or_else(|| PathBuf::from("."), PathBuf::from)
}

pub fn spec(target: &str) -> HostSpec {
    let home = home();
    let file = std::env::var_os("MAI_SSH_CONFIG")
        .map_or_else(|| home.join(".ssh").join("config"), PathBuf::from);
    let cfg = parse_config(&std::fs::read_to_string(file).unwrap_or_default()).expect("ssh config");
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    resolve(&cfg, target, &user, &home).expect("resolve target")
}

pub fn options() -> ConnectOptions {
    let learn_to = std::env::var_os("MAI_LEARN_TO").map_or_else(
        || std::env::temp_dir().join("mai-example-known_hosts"),
        PathBuf::from,
    );
    let mut known_hosts = vec![learn_to.clone()];
    if std::env::var_os("MAI_SKIP_USER_KNOWN_HOSTS").is_none() {
        known_hosts.insert(0, home().join(".ssh").join("known_hosts"));
    }
    ConnectOptions {
        known_hosts,
        learn_to,
        timeout: Duration::from_secs(20),
        use_agent: true,
    }
}

pub async fn connect_or_exit(target: &str) -> SshSession<TermPrompter> {
    let spec = spec(target);
    println!(
        "connecting: {}@{}:{} (jumps: {})",
        spec.user,
        spec.host,
        spec.port,
        spec.jumps.len()
    );
    match connect(
        &spec,
        &options(),
        Arc::new(TermPrompter),
        &MemoryStore::default(),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
```

- [ ] **Step 7: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: `ssh_session` 24 个测试通过，全部 269 个；clippy 无警告。

- [ ] **Step 8: 提交**

```bash
git add crates/mai-core
git commit -m "fix(core): tests never reach the real agent; move old passphrase entries; readable signer error

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: 上传超时、跳板主机提示、失败时的提示（B34、B35、B37）

**Files:**

- Modify: `crates/mai-core/src/ssh/client.rs`、`crates/mai-core/src/deploy.rs`、
  `crates/mai-core/src/ssh/config.rs`、`crates/mai-core/src/connect.rs`
- Test: `crates/mai-core/tests/deploy.rs`、`crates/mai-core/tests/ssh_config.rs`、
  `crates/mai-core/tests/connect.rs`

**Interfaces:**

- Consumes: Task 4 的 `connect.rs`、Task 5 的 `client.rs`。
- Produces:

  ```rust
  // ssh::client
  pub const SFTP_REQUEST_TIMEOUT_SECS: u64 = 60;
  // deploy
  pub fn upload_error(e: impl Into<std::io::Error>) -> DeployError
      // TimedOut -> DeployError::Ssh(SshError::Connect("upload timed out: ..")) (Retry)
  // ssh::config
  pub fn jump_warnings(config: &SshConfig, spec: &HostSpec) -> Vec<String>
  // connect（替换 with_config_warnings）
  pub fn with_notes(err: OpenError, notes: &[String]) -> OpenError
  ```

  `with_notes` 把 `" (n1; n2)"` 附加到 `Retry` 的原因与 `Auth`、`Deploy`、
  `Config` 的文本上；其他问题不变；没有提示时不变。

- [ ] **Step 1: 写失败测试**

`crates/mai-core/tests/deploy.rs`（完整内容）：

```rust
use std::path::PathBuf;

use mai_core::deploy::{
    DeployError, HookResult, Os, ProbeStore, Remote, Shell, hooks_result, marked_lines,
    normalize_arch, parse_hash, parse_hook_lines, parse_posix_detect, parse_uname,
    parse_windows_detect, posix_detect_command, sha256_hex, stop_args, upload_error,
    windows_detect_command,
};
use mai_core::ssh::client::{ExecOutput, SshError};

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
fn powershell_output_of_the_detection_command_is_not_posix() {
    // Real Windows PowerShell output when the markers are echoed but
    // `uname` is missing (the old, unguarded command).
    let out = "MAI-DETECT-BEGIN\r\nMAI-DETECT-END\r\n";
    assert_eq!(parse_posix_detect(out).unwrap(), None);
    // The chain only prints markers when uname works, and has no braces
    // (fish and PowerShell on Unix cannot parse them).
    let cmd = posix_detect_command();
    assert!(
        cmd.starts_with("uname -sm >/dev/null && echo MAI-DETECT-BEGIN && "),
        "{cmd}"
    );
    assert!(!cmd.contains('{'), "{cmd}");
    assert!(!cmd.contains('\n'), "{cmd:?}");
    assert!(cmd.contains(r"printf '%s\n'"), "{cmd}");
}

#[test]
fn windows_uname_in_a_marked_block_is_not_posix() {
    let out = "MAI-DETECT-BEGIN\nMINGW64_NT-10.0-26300 x86_64\n/c/Users/u\nMAI-DETECT-END\n";
    assert_eq!(parse_posix_detect(out).unwrap(), None);
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

/// B34: an upload that times out on a slow link is a network problem
/// (retried); other upload failures need the user.
#[test]
fn upload_timeouts_are_retried_and_other_failures_need_the_user() {
    use std::io;
    let timed_out = |e: DeployError| matches!(&e, DeployError::Ssh(SshError::Connect(m)) if m.contains("timed out"));
    assert!(timed_out(upload_error(io::Error::new(
        io::ErrorKind::TimedOut,
        "Timeout"
    ))));
    // What russh-sftp reports when a request gets no answer in time.
    assert!(timed_out(upload_error(
        russh_sftp::client::error::Error::Timeout
    )));
    assert_eq!(
        upload_error(io::Error::other("Permission denied")),
        DeployError::Upload("Permission denied".into())
    );
}
```

`crates/mai-core/tests/ssh_config.rs`（完整内容）：

```rust
use std::path::{Path, PathBuf};

use mai_core::ssh::config::{config_warnings, jump_warnings, parse_config, resolve};

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
fn include_lines_are_reported() {
    assert_eq!(
        config_warnings("Host a\n  User x\nInclude conf.d/*\n"),
        vec![
            "ssh config line 3: Include is followed, but Match blocks in included files are not detected; their settings may apply to the preceding Host"
                .to_owned()
        ]
    );
}

#[test]
fn bad_port_is_an_error() {
    let cfg = parse_config("").unwrap();
    assert!(resolve(&cfg, "host:notaport", "me", &home()).is_err());
}

/// B35: a jump host's own ProxyJump, which is not followed, is reported.
#[test]
fn jump_hosts_own_proxyjump_is_reported() {
    let cfg = parse_config(CONFIG).unwrap();
    let looped = resolve(&cfg, "loop-a", "me", &home()).unwrap();
    assert_eq!(
        jump_warnings(&cfg, &looped),
        vec!["ssh config: ProxyJump loop-a of jump host loop-b is not followed"]
    );
    let plain = resolve(&cfg, "box", "me", &home()).unwrap();
    assert!(
        jump_warnings(&cfg, &plain).is_empty(),
        "mac has no ProxyJump"
    );
}
```

`crates/mai-core/tests/connect.rs`（完整内容）：

```rust
//! Pure parts of the real connector: error classification, local
//! platform detection and binary placement.

use std::cell::Cell;
use std::path::PathBuf;

use mai_core::connect::{
    deploy_open_error, local_remote, place_binary, ssh_open_error, with_notes,
};
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

#[test]
fn notes_are_added_to_failures_with_text() {
    let notes = vec!["w1".to_owned(), "w2".to_owned()];
    let auth = OpenError::NeedsUser(Problem::Auth("denied".into()));
    assert_eq!(
        with_notes(auth.clone(), &notes),
        OpenError::NeedsUser(Problem::Auth("denied (w1; w2)".into()))
    );
    assert_eq!(with_notes(auth.clone(), &[]), auth);
    assert_eq!(
        with_notes(OpenError::Retry("refused".into()), &notes),
        OpenError::Retry("refused (w1; w2)".into())
    );
    assert_eq!(
        with_notes(OpenError::NeedsUser(Problem::Deploy("x".into())), &notes),
        OpenError::NeedsUser(Problem::Deploy("x (w1; w2)".into()))
    );
    assert_eq!(
        with_notes(OpenError::NeedsUser(Problem::Config("c".into())), &notes),
        OpenError::NeedsUser(Problem::Config("c (w1; w2)".into()))
    );
    let key = OpenError::NeedsUser(Problem::HostKeyRejected);
    assert_eq!(with_notes(key.clone(), &notes), key, "no text to add to");
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test deploy --test ssh_config --test connect`
Expected: 编译失败（`upload_error`、`jump_warnings`、`with_notes` 不存在）。

- [ ] **Step 3: `crates/mai-core/src/ssh/client.rs`（最终完整内容）**

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
    /// Offer the keys of the running ssh-agent (or Pageant on Windows).
    /// Tests turn this off so they never reach the developer's agent.
    pub use_agent: bool,
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
    authenticate(
        &mut handle,
        hop,
        prompter.as_ref(),
        secrets,
        opts.use_agent,
        &mut notes,
    )
    .await?;
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
    use_agent: bool,
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
                    let mut signer = FileSigner {
                        path,
                        prompter,
                        secrets,
                        notes,
                        declined: false,
                    };
                    let r = match h
                        .authenticate_publickey_with(user, public.clone(), hash, &mut signer)
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => return Err(connect_err(e)),
                    };
                    // A declined passphrase leaves the key to the agent.
                    if !signer.declined {
                        tried.push(public);
                    }
                    r
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
        if use_agent && try_agent(h, user, &tried, &mut methods).await? == Step::Done {
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

/// How long one SFTP request may take. russh-sftp's default (10 s) is too
/// short for writing a probe binary over a slow link (B34).
pub const SFTP_REQUEST_TIMEOUT_SECS: u64 = 60;

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

    /// Whether the connection is gone (the session task has ended).
    pub fn is_closed(&self) -> bool {
        self.handle.is_closed()
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
        let cfg = russh_sftp::client::Config {
            request_timeout_secs: SFTP_REQUEST_TIMEOUT_SECS,
            ..Default::default()
        };
        russh_sftp::client::SftpSession::new_with_config(ch.into_stream(), cfg)
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
/// It is a flat `&&` chain, with no braces, so that fish and PowerShell
/// on Unix (which cannot parse `{ ...; }`) still run it. The markers are
/// printed only when `uname` works: Windows PowerShell 5.1 rejects `&&`
/// as a syntax error and cmd fails on the `/dev/null` redirect, so
/// neither prints a marked block (a plain `echo` of the markers would
/// succeed under PowerShell even though `uname` fails). Other shells run
/// the chain as written.
pub fn posix_detect_command() -> String {
    format!(
        "uname -sm >/dev/null && echo {BEGIN_MARK} && uname -sm && printf '%s\\n' \"$HOME\" && echo {END_MARK}"
    )
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
/// the output is not from a POSIX shell (no markers, an empty marked
/// block, or a `uname` naming MINGW, MSYS or CYGWIN: a Windows host whose
/// login shell found Git's `uname` on PATH).
pub fn parse_posix_detect(out: &str) -> Result<Option<(Os, String, String)>, DeployError> {
    let Some(lines) = marked_lines(out) else {
        return Ok(None);
    };
    if lines.is_empty() {
        return Ok(None);
    }
    let [uname, home] = lines.as_slice() else {
        return Err(DeployError::Detect(format!(
            "unexpected detection output: {lines:?}"
        )));
    };
    let kernel = uname.to_ascii_uppercase();
    if ["MINGW", "MSYS", "CYGWIN"]
        .iter()
        .any(|p| kernel.starts_with(p))
    {
        return Ok(None);
    }
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

/// An upload step failed. A timeout (a slow link) is a network problem and
/// is retried like other network errors (design 3.4, B34); anything else
/// needs the user.
pub fn upload_error(e: impl Into<std::io::Error>) -> DeployError {
    let e: std::io::Error = e.into();
    if e.kind() == std::io::ErrorKind::TimedOut {
        DeployError::Ssh(SshError::Connect(format!("upload timed out: {e}")))
    } else {
        DeployError::Upload(e.to_string())
    }
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
        if !sftp.try_exists(path.clone()).await.map_err(upload_error)? {
            sftp.create_dir(path.clone()).await.map_err(upload_error)?;
        }
    }
    let target = format!("{bin_dir}/{}", remote.exe_name());
    let tmp = format!("{target}.upload");
    let mut file = sftp.create(tmp.clone()).await.map_err(upload_error)?;
    file.write_all(bytes).await.map_err(upload_error)?;
    file.shutdown().await.map_err(upload_error)?;
    if remote.os != Os::Windows {
        let attrs = russh_sftp::protocol::FileAttributes {
            permissions: Some(0o755),
            ..Default::default()
        };
        sftp.set_metadata(tmp.clone(), attrs)
            .await
            .map_err(upload_error)?;
    }
    let tmp_abs = format!("{}.upload", remote.probe_path(dir));
    let out = s.exec(&remote.hash_command(&tmp_abs)).await?;
    let Some(written) = parse_hash(&out.stdout_str()) else {
        let _ = sftp.remove_file(tmp).await;
        return Err(DeployError::Upload(format!(
            "cannot compute SHA-256 of {tmp_abs} on the host (exit {:?}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    };
    if written != sha256_hex(bytes) {
        let _ = sftp.remove_file(tmp).await;
        return Err(DeployError::Upload(format!(
            "uploaded file does not match (SHA-256 {written})"
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

- [ ] **Step 5: `crates/mai-core/src/ssh/config.rs`（完整内容）**

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

/// Things in `text` the user should know about, with 1-based line
/// numbers: one entry per `Match` block (ignored) and per `Include` (the
/// parser follows it, but cannot see `Match` blocks in the included file).
pub fn config_warnings(text: &str) -> Vec<String> {
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| match keyword(l).as_str() {
            "match" => Some(format!(
                "ssh config line {}: Match blocks are not supported; their settings are ignored",
                i + 1
            )),
            "include" => Some(format!(
                "ssh config line {}: Include is followed, but Match blocks in included files are not detected; their settings may apply to the preceding Host",
                i + 1
            )),
            _ => None,
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
    spec.jumps = jumps_of(config, target)?
        .iter()
        .map(|j| resolve_hop(config, j, default_user, home))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(spec)
}

/// The `ProxyJump` hosts configured for `target`, outermost first.
fn jumps_of(config: &SshConfig, target: &str) -> Result<Vec<String>, ConfigError> {
    let params = config.query(split_target(target)?.1);
    Ok(params
        .proxy_jump
        .unwrap_or_default()
        .iter()
        .flat_map(|j| j.split(','))
        .map(str::trim)
        .filter(|j| !j.is_empty() && !j.eq_ignore_ascii_case("none"))
        .map(str::to_owned)
        .collect())
}

/// One note per jump host of `spec` that has a `ProxyJump` of its own:
/// it is not followed (design 3.1), which would otherwise only show as
/// failing connects (B35).
pub fn jump_warnings(config: &SshConfig, spec: &HostSpec) -> Vec<String> {
    spec.jumps
        .iter()
        .filter_map(|j| {
            let own = jumps_of(config, &j.alias).ok()?;
            (!own.is_empty()).then(|| {
                format!(
                    "ssh config: ProxyJump {} of jump host {} is not followed",
                    own.join(","),
                    j.alias
                )
            })
        })
        .collect()
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

- [ ] **Step 6: `crates/mai-core/src/connect.rs`（最终完整内容）**

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
use crate::ssh::config::{HostSpec, config_warnings, jump_warnings, parse_config, resolve};
use crate::stderr::{StderrTail, collect as collect_stderr};
use crate::swap::{LocalFiles, swap_in};
use crate::terminals::{SystemTerminals, find_zellij_command, found_zellij};

/// Locale requested for `serve` so zellij output is UTF-8.
const LOCALE: &str = "en_US.UTF-8";

/// Append the connect notes (ignored ssh config, keychain failures) to a
/// failure's text, so they are not lost when the connect fails (B37): an
/// ignored `Match` block may be why authentication failed, a dropped
/// `ProxyJump` why the host cannot be reached. Problems without text
/// (host keys, protocol) are unchanged.
pub fn with_notes(err: OpenError, notes: &[String]) -> OpenError {
    if notes.is_empty() {
        return err;
    }
    let add = |m: String| format!("{m} ({})", notes.join("; "));
    match err {
        OpenError::Retry(m) => OpenError::Retry(add(m)),
        OpenError::NeedsUser(Problem::Auth(m)) => OpenError::NeedsUser(Problem::Auth(add(m))),
        OpenError::NeedsUser(Problem::Deploy(m)) => OpenError::NeedsUser(Problem::Deploy(add(m))),
        OpenError::NeedsUser(Problem::Config(m)) => OpenError::NeedsUser(Problem::Config(add(m))),
        other => other,
    }
}

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

    /// The resolved target and the config warnings (ignored `Match`
    /// blocks, `Include`, jump hosts' own `ProxyJump`).
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
        let mut warnings = config_warnings(&text);
        warnings.extend(jump_warnings(&config, &spec));
        Ok((spec, warnings))
    }

    async fn open_ssh(&self, host: &HostConfig, target: &str) -> Result<Opened, OpenError> {
        let (spec, mut notes) = self.spec(target)?;
        let gate = self.gate(&host.id);
        let guard = gate.lock().await;
        let session = connect(&spec, &self.opts, self.prompter.clone(), &*self.secrets)
            .await
            .map_err(|e| with_notes(ssh_open_error(e), &notes))?;
        notes.extend(session.notes().iter().cloned());
        let opts = DeployOptions {
            dir: self.dir.clone(),
            install_hooks: false,
            client: self.client.clone(),
        };
        let report = deploy(&session, &self.probes, &opts)
            .await
            .map_err(|e| with_notes(deploy_open_error(e), &notes))?;
        let remote = &report.remote;
        let hooks = if self.install_hooks {
            let cmd = remote.invoke(&report.probe_path, "install-hooks");
            let out = session
                .exec(&cmd)
                .await
                .map_err(|e| with_notes(ssh_open_error(e), &notes))?;
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
            .map_err(|e| with_notes(ssh_open_error(e), &notes))?;
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
        let (spec, notes) = self.spec(target)?;
        let gate = self.gate(&host.id);
        let _guard = gate.lock().await;
        let session = connect(&spec, &self.opts, self.prompter.clone(), &*self.secrets)
            .await
            .map_err(|e| with_notes(ssh_open_error(e), &notes))?;
        let remote = detect(&session)
            .await
            .map_err(|e| with_notes(deploy_open_error(e), &notes))?;
        // Used when neither the host config nor the probe names a zellij
        // (the probe may be down); a failed search just leaves PATH.
        let found = if remote.shell == Shell::Posix {
            match session.exec(&find_zellij_command()).await {
                Ok(out) => found_zellij(&out.stdout_str()),
                Err(_) => None,
            }
        } else {
            None
        };
        Ok(SystemTerminals::Ssh {
            session,
            remote,
            found,
        })
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

- [ ] **Step 7: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: `deploy` 20 个、`ssh_config` 11 个、`connect` 6 个测试通过，全部 271 个；
clippy 无警告。另检查源码只含 ASCII：

```bash
LC_ALL=C grep -rn '[^[:print:][:space:]]' crates/*/src || echo ascii-ok
```

Expected: `ascii-ok`。

- [ ] **Step 8: 提交**

```bash
git add crates/mai-core
git commit -m "fix(core): retry upload timeouts; note ignored jump-host ProxyJump; keep notes on failure

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: 端到端验证与文档

**Files:**

- Modify: `README.md`、`docs/superpowers/plans/2026-09-24-02-followups.md`

- [ ] **Step 1: Windows 本机终端端到端**

zellij 不在 PATH 时用 `MAI_E2E_ZELLIJ` 指定（本机为 `D:\Apps\zellij\zellij.exe`）。
先确认 `%USERPROFILE%\.mai-e2e` 不存在，`zellij list-sessions -n` 里没有
`mai-e2e-03e`：

```bash
cargo build -p mai-probe --release --locked
mkdir -p probes/x86_64-pc-windows-msvc
cp target/release/mai-probe.exe probes/x86_64-pc-windows-msvc/
MAI_E2E_ZELLIJ='D:\Apps\zellij\zellij.exe' cargo run -p mai-core --example term -- probes local mai-e2e-03e
```

Expected（退出码 0）：

```text
hello: Ok(Some(HelloInfo { .. zellij_path: Some("D:\\Apps\\zellij\\zellij.exe") .. }))
first event: None; <数万> bytes drawn
command output seen: true
after Ctrl+q: Some(Exited(Some(0)))
```

清理（session 已被 Ctrl+q 删除时报 not found 属正常）：

```bash
"D:/Apps/zellij/zellij.exe" kill-session mai-e2e-03e
"D:/Apps/zellij/zellij.exe" delete-session --force mai-e2e-03e
"D:/Apps/zellij/zellij.exe" list-sessions -n
```

```powershell
Get-Process mai-probe -ErrorAction SilentlyContinue | Where-Object { $_.Path -like '*\.mai-e2e\*' } | Stop-Process -Force
Remove-Item -Recurse -Force -LiteralPath "$env:USERPROFILE\.mai-e2e" -ErrorAction SilentlyContinue
```

`list-sessions` 只应剩下用户原有的 session。

- [ ] **Step 2: 更新 `README.md`**

`## mai-core` 小节：

- `HostManager::open_terminal` 一条改为：

```markdown
- `HostManager::open_terminal` attaches a terminal to a zellij session
  (`zellij attach [--create]`) in a PTY: over a per-host terminal SSH
  connection, or a local PTY (ConPTY on Windows). If the connection drops,
  terminals get `Detached` and are reattached automatically; a single closed
  channel reattaches only that terminal. Session names that are empty or
  start with `-` are refused. Without a zellij path from the host config or
  the probe, zellij is searched for on the host (PATH, common install dirs,
  login shell).
```

- `ssh::client::connect` 一条末尾加一句：

```markdown
  `ConnectOptions::use_agent` turns the ssh-agent step off (tests do).
```

- `deploy::deploy` 一条末尾加一句：

```markdown
  Upload timeouts on slow links are retried like other network errors.
```

- [ ] **Step 3: 更新 `docs/superpowers/plans/2026-09-24-02-followups.md`**

- “03e 必须处理”小节标题改为“03e 之后待处理”。
- 以下各条末尾加“**（已在 03e 处理）**”：B26、B27、B28、B29、B30、B31、B32、
  B34、B35、B37、B38、B39、B40。
- B36 末尾加：“**（03e：用户决定保持严格，设计 3.3 已注明）**”。
- 在该小节末尾新增：
  - **B41 Ubuntu 上终端端到端失败**：03e 验证时（Ubuntu，zellij 0.44.1，
    慢链路），`term` 示例 attach 新 session 后能看到 zellij 界面与
    “About Zellij” 提示弹窗，ESC 能关闭弹窗、出现 shell 提示符，但随后输入的
    `echo` 命令没有回显，Ctrl+q 也未在 15 秒内结束；main（03d）上同样失败，
    不是 03e 引入的。待查：输入时序（shell 尚未就绪）、该主机 zellij 的按键
    配置、慢链路下的 PTY 行为。与 B25 一起观察。

Run: `npx -y markdownlint-cli2 README.md docs/superpowers/specs/*.md docs/superpowers/plans/*.md`
Expected: 0 issues。

- [ ] **Step 4: 提交、推送分支、等待 CI**

```bash
git add README.md docs
git commit -m "docs: 03e terminal hardening; follow-ups

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u origin feat/03e-terminal-hardening
gh run list --repo phalanger/multi-ai --branch feat/03e-terminal-hardening --limit 1
gh run watch <run-id> --repo phalanger/multi-ai --exit-status
```

Expected: CI 三平台测试与 5 个探针目标全部通过（Linux/macOS 上 `terminals` 的
3 个 `#[cfg(unix)]` 测试在真实 `/bin/sh` 下运行查找脚本）。

- [ ] **Step 5: Ubuntu 部署端到端（上传超时设置）**

```bash
gh run download <branch-run-id> --repo phalanger/multi-ai --name probes --dir probes-new
ssh ubuntu@100.66.61.30 'ls -d ~/.mai-e2e 2>/dev/null || echo none'
cargo run -p mai-core --example monitor -- probes-new 240 ubuntu@100.66.61.30
ssh ubuntu@100.66.61.30 'ls ~/.mai-e2e/bin; sha256sum ~/.mai-e2e/bin/mai-probe'
sha256sum probes-new/x86_64-unknown-linux-musl/mai-probe
ssh ubuntu@100.66.61.30 'pgrep -x mai-probe -l || echo none; rm -rf ~/.mai-e2e; ls -d ~/.mai 2>/dev/null || echo "no ~/.mai"'
```

Expected：第一条 ssh 输出 `none`（否则停止并报告）；出现 `Hello { .. os: "linux" .. }`
与 `Host { .. state: Online, probe: Up .. }`（上传一次完成，不再出现
`upload probe: Timeout`）；`ls` 只有 `mai-probe`；两个 SHA-256 相同；清理后为
`none`、`no ~/.mai`。本机删除 `probes/`、`probes-new/`。

## 已知限制（留给 04）

- 终端连接期间的打开请求按顺序处理；一个慢的连接（如等待主机密钥确认）会让后续
  打开排队。
- Ubuntu 上终端端到端失败（B41），原因未明；B25、B33 仍待观察。
- 需要特定环境的人工验证：Windows sshd（B1、B20）、ssh-agent 与 2FA（B2、B15）、
  `KeyringStore` 真实使用（B3）。
