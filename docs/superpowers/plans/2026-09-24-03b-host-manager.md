# 03b 主机管理与探针连接 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让应用能长期监控多台主机（含本机）：每台主机一个任务负责连接、部署、
启动 `mai-probe serve`、转发消息与退避重连；所有主机的事件汇总到一个 `Tracker`，
产出供 UI 使用的 `Update`（主机状态、session/pane、agent 状态、提醒、指标）。
同时修掉探针在长时间运行下的问题（02 遗留 C1-C4、C6、C7、S1、S2）。

**Architecture:** 探针侧：zellij 命令超时、按对象只报一次错误、spool 零点宽限与
定期清理、按 client 区分的 ack 与单实例（pid 文件 + `stop` 子命令）、数据目录跟随
二进制位置、抓屏绑定的保持与解除。应用侧（`mai-core`）：`link`（JSON Lines 握手、
静默检测）、`host`（每主机任务、状态、退避）、`monitor`（纯函数汇总）、
`manager`（任务编排与对外句柄）、`connect`（真实连接器：SSH 与本机子进程）。
核心逻辑用内存管道与脚本化连接器测试，时间用 tokio 暂停时钟。

**Tech Stack:** Rust 1.92（edition 2024）、tokio（测试加 `test-util`）、russh 0.63、
sysinfo 0.38（探针结束旧进程）、既有的 mai-protocol / mai-probe / mai-core。

**Spec:** `docs/superpowers/specs/2026-09-23-multi-ai-monitor-design.md`
（第 2.2、3.4、4.1、4.2、4.3、4.5、4.6、5、6.2 节，已按本计划更新）；
遗留事项 `docs/superpowers/plans/2026-09-24-02-followups.md`。

## Global Constraints

- 源码（代码、字符串、注释）只含 ASCII；测试、示例与文档可用中文。
- 零警告：`cargo build` 无警告；
  `cargo clippy --workspace --all-targets --locked -- -D warnings` 通过。
- 本计划给出的每个文件内容都已在 Windows 上编译、测试（最终 190 个测试），
  并在 CI 三平台（Windows、macOS、Linux）通过、5 个探针目标构建成功；
  用 CI 产出的探针对 Mac（SSH）与 Windows 本机做过端到端验证。**逐字写入**。
  用编辑器或文件写入工具写，**不要用 Git Bash 的 heredoc**（会把 `\\` 压成 `\`）。
- 格式：本计划的文件已按 `rustfmt --edition 2024` 格式化；不要对任何 `lib.rs`
  或 `tracker.rs` 运行 rustfmt（会连带改动其他模块）。
- 在 Git Bash 中运行以 `/` 开头的参数时设 `MSYS_NO_PATHCONV=1`。
- 测试不得连接外部主机，不得读写真实的系统钥匙串、`~/.ssh`、`~/.mai`、
  `~/.claude`、`~/.codex`。
- 远程主机（Mac、Ubuntu）只用于测试：**不在远程主机上构建或打包**；
  远程端到端验证使用 GitHub Actions `probes` 产物中的探针。
- 每次提交信息末尾空一行加：
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
- 分支：`feat/03b-host-manager`，从 `main` 创建；本项目只有一名开发者，
  完成后本地合并到 `main` 并直接推送，不开 PR。

## Review Focus

- **shell 启动文件往 stdout 打印内容**：用户期望照常连上。`Hello` 之前容忍最多
  50 行非协议输出（Task 4 `hello_is_found_after_shell_noise`、
  `endless_noise_is_not_a_probe`）。
- **同一台主机被两个应用（如 Mac 与 Windows 上的应用）同时监控**：互不结束对方的
  `serve`，重启后不吞掉对方的事件（Task 1 `each_client_has_its_own_ack`，Task 2
  `serves_of_different_clients_coexist`）。
- **zellij 命令挂起**：`serve` 不能卡住，也不能每个周期重复报同一个错
  （Task 1 `hung_command_is_killed_at_timeout`、
  `persistent_zellij_error_is_reported_once_per_failure`）。
- **网络断了但连接没有关闭（半开）**：应用 20 秒内发现并重连
  （Task 4 `silence_after_hello_is_detected`，Task 5 重连测试）。
- **Claude 弹出不匹配任何屏幕特征的确认框**：不能被判为退出并丢失 `NeedsInput`
  （Task 1 `prompt_matching_only_needs_input_rules_keeps_scrape_binding`）。

## 02 遗留事项的处理

| 编号 | 处理 |
| --- | --- |
| C1 | zellij 命令 5 秒超时并结束子进程；周期性错误按对象只报一次（Task 1） |
| C2 | `serve` 每小时检查并删除超过 7 天的 spool 文件（Task 1） |
| C3 | 数据目录跟随已部署的二进制位置（Task 2） |
| C4 | 单实例：pid 文件、`stop`、替换前先停（Task 1、2、3、7） |
| C5、B10 | 每台主机一个任务，连接与部署天然串行；按主机的锁供 03c 的终端连接共用（Task 5、7） |
| C6 | 新一天的 spool 文件在零点后 10 秒内不读（Task 1） |
| C7 | 本机探针与探针启动的 zellij 命令均用 `CREATE_NO_WINDOW`（Task 1、7） |
| S1、S2 | 抓屏绑定的保持与解除（Task 1） |
| S4 | 设计文档 4.5 已注明显式 `--zellij` 不存在时不回退（随本计划提交） |
| B1-B4、B5-B9、B11-B17、S3、S5 | 不在本计划；B 类归 03c，S3、S5 归 04 |
| B18 | 部分处理：替换前先 `stop`；另一个 client 的 `serve` 仍会锁住文件（见已知限制） |

## 文件结构

```text
crates/mai-protocol/src/lib.rs          + sanitize_client（client id 只留 [A-Za-z0-9_-]）
crates/mai-probe/
  src/zellij.rs                         命令超时、CREATE_NO_WINDOW
  src/spool.rs                          零点宽限、按 client 的 ack、临时文件名
  src/scrape.rs                         + CompiledRules::still_matches
  src/serve.rs                          错误只报一次、定期清理、S1/S2
  src/instance.rs (新)                  数据目录、pid 文件、结束旧 serve
  src/install.rs                        临时文件名带进程号
  src/main.rs                           serve --client、stop 子命令、数据目录
  src/lib.rs                            + instance
crates/mai-core/
  Cargo.toml                            dev: tokio test-util
  src/deploy.rs                         serve/stop 参数、替换前 stop、DeployOptions.client
  src/link.rs (新)                      ProbeLink：握手、收发、静默检测
  src/host.rs (新)                      HostConfig、ConnState、HostState、Backoff、run_host
  src/monitor.rs (新)                   Monitor：汇总为 Update
  src/manager.rs (新)                   HostManager：任务编排
  src/connect.rs (新)                   SystemConnector：SSH 与本机
  src/tracker.rs                        + records()
  examples/monitor.rs (新)              手工端到端验证
```

---

### Task 1: 探针 serve 循环的可靠性

**Files:**

- Modify: `crates/mai-protocol/src/lib.rs`、`crates/mai-probe/src/zellij.rs`、
  `crates/mai-probe/src/spool.rs`、`crates/mai-probe/src/scrape.rs`、
  `crates/mai-probe/src/serve.rs`
- Test: `crates/mai-probe/tests/zellij.rs`、`crates/mai-probe/tests/spool.rs`、
  `crates/mai-probe/tests/serve.rs`

**Interfaces:**

- Produces:
  - `mai_protocol::sanitize_client(&str) -> String`（只保留 `[A-Za-z0-9_-]`）。
  - `zellij::COMMAND_TIMEOUT: Duration`（5 秒）、
    `zellij::no_window(&mut Command) -> &mut Command`、
    `zellij::output_with_timeout(&mut Command, Duration) -> io::Result<Option<Output>>`
    （超时返回 `Ok(None)` 并结束子进程）。
  - `spool::DAY_MS`、`spool::DAY_GRACE_MS`（10 000）、
    `Spool::for_client(dir, client)`（ack 文件 `ack-<client>`；client 清洗后为空则同
    `Spool::new`）、`Spool::read_after(cursor, now_ms)`（**新增 `now_ms` 参数**）、
    `spool::sanitize_client`（重新导出）。
  - `CompiledRules::still_matches(agent, title, command, screen) -> bool`。
  - `serve::SPOOL_KEEP_DAYS`（7）、`serve::CLEANUP_EVERY_MS`（3 600 000）、
    `serve::MISSES_TO_EXIT`（2）；`Server::tick` 负责定期清理。

- [ ] **Step 1: 更新测试**

`crates/mai-probe/tests/zellij.rs` 完整内容：

```rust
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use mai_probe::zellij::{
    default_dirs, find_zellij_in, output_with_timeout, parse_panes, parse_sessions, parse_version,
    sessions_from_output,
};
use mai_protocol::{PaneInfo, SessionInfo};

#[test]
fn sessions_parse_names_and_exited_flag() {
    let text = "work [Created 2months 6days ago] (current)\n\
                mai-spike-u5 [Created 14m 46s ago] (EXITED - attach to resurrect)\n\
                \n";
    assert_eq!(
        parse_sessions(text),
        vec![
            SessionInfo {
                name: "work".into(),
                exited: false
            },
            SessionInfo {
                name: "mai-spike-u5".into(),
                exited: true
            },
        ]
    );
}

#[test]
fn no_sessions_message_means_empty_list() {
    let msg = "No active zellij sessions found.\n";
    assert_eq!(sessions_from_output(false, msg, ""), Ok(vec![]));
    assert_eq!(sessions_from_output(true, msg, ""), Ok(vec![]));
    assert_eq!(sessions_from_output(false, "", msg), Ok(vec![]));
}

#[test]
fn sessions_output_success_parses_and_failure_is_error() {
    assert_eq!(
        sessions_from_output(true, "work [Created 1h ago] (current)\n", ""),
        Ok(vec![SessionInfo {
            name: "work".into(),
            exited: false
        }])
    );
    let err = sessions_from_output(false, "", "permission denied").unwrap_err();
    assert!(err.0.contains("permission denied"), "{err}");
}

#[test]
fn pane_with_only_required_fields_parses() {
    let panes = parse_panes(r#"[{"id": 2, "is_plugin": false}]"#).unwrap();
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].id, 2);
    assert_eq!(panes[0].title, "");
    assert!(!panes[0].exited);
}

/// Shape of `zellij action list-panes -a -j` (zellij 0.44.3), trimmed to
/// one plugin, one idle shell and one pane running a command.
const PANES_JSON: &str = r#"[
  {"id": 0, "is_plugin": true, "is_focused": false, "is_fullscreen": false,
   "is_floating": false, "is_suppressed": false, "title": "zellij:tab-bar",
   "exited": false, "exit_status": null, "is_held": false, "pane_x": 0,
   "pane_content_x": 0, "pane_y": 0, "pane_content_y": 0, "pane_rows": 1,
   "pane_content_rows": 1, "pane_columns": 80, "pane_content_columns": 80,
   "cursor_coordinates_in_pane": null, "terminal_command": null,
   "plugin_url": "zellij:tab-bar", "is_selectable": false,
   "index_in_pane_group": {}, "default_fg": null, "default_bg": null,
   "tab_id": 0, "tab_position": 0, "tab_name": "main"},
  {"id": 3, "is_plugin": false, "is_focused": true, "is_fullscreen": false,
   "is_floating": false, "is_suppressed": false,
   "title": "C:\\WINDOWS\\system32\\cmd.exe", "exited": false,
   "exit_status": null, "is_held": false, "pane_x": 0, "pane_content_x": 1,
   "pane_y": 1, "pane_content_y": 2, "pane_rows": 20, "pane_content_rows": 18,
   "pane_columns": 80, "pane_content_columns": 78,
   "cursor_coordinates_in_pane": [0, 0], "terminal_command": null,
   "plugin_url": null, "is_selectable": true, "index_in_pane_group": {},
   "default_fg": null, "default_bg": null, "tab_id": 0, "tab_position": 0,
   "tab_name": "main", "pane_cwd": "C:\\work"},
  {"id": 14, "is_plugin": false, "is_focused": false, "is_fullscreen": false,
   "is_floating": false, "is_suppressed": false, "title": "Claude Code",
   "exited": false, "exit_status": null, "is_held": false, "pane_x": 0,
   "pane_content_x": 1, "pane_y": 21, "pane_content_y": 22, "pane_rows": 20,
   "pane_content_rows": 18, "pane_columns": 80, "pane_content_columns": 78,
   "cursor_coordinates_in_pane": null,
   "terminal_command": "C:\\WINDOWS\\system32\\cmd.exe",
   "plugin_url": null, "is_selectable": true, "index_in_pane_group": {},
   "default_fg": null, "default_bg": null, "tab_id": 2, "tab_position": 1,
   "tab_name": "agents", "pane_command": "claude.exe -c",
   "pane_cwd": "G:\\work"}
]"#;

#[test]
fn panes_skip_plugins_and_prefer_running_command() {
    assert_eq!(
        parse_panes(PANES_JSON).unwrap(),
        vec![
            PaneInfo {
                id: 3,
                tab_id: 0,
                tab_name: "main".into(),
                title: "C:\\WINDOWS\\system32\\cmd.exe".into(),
                command: None,
                exited: false,
            },
            PaneInfo {
                id: 14,
                tab_id: 2,
                tab_name: "agents".into(),
                title: "Claude Code".into(),
                command: Some("claude.exe -c".into()),
                exited: false,
            },
        ]
    );
}

#[test]
fn panes_reject_bad_json() {
    assert!(parse_panes("{").is_err());
}

#[test]
fn version_parses_second_token() {
    assert_eq!(parse_version("zellij 0.44.3\n").as_deref(), Some("0.44.3"));
    assert_eq!(parse_version(""), None);
}

fn touch(path: &std::path::Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"").unwrap();
}

/// Uses temp dirs only, so the result does not depend on whether the host
/// has zellij installed in a common location (e.g. Homebrew on macOS).
#[test]
fn find_prefers_explicit_then_path_then_candidate_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    let in_path = tmp.path().join("bin").join("zellij");
    let first_dir = tmp.path().join("first");
    let second_dir = tmp.path().join("second");
    let in_second = second_dir.join("zellij");
    let explicit = tmp.path().join("custom").join("zellij.exe");
    touch(&in_path);
    touch(&in_second);
    touch(&explicit);
    let path_env: OsString =
        std::env::join_paths([tmp.path().join("empty"), tmp.path().join("bin")]).unwrap();
    let dirs = [first_dir, second_dir];

    assert_eq!(
        find_zellij_in(Some(&explicit), Some(&path_env), &dirs),
        Some(explicit.clone())
    );
    assert_eq!(find_zellij_in(None, Some(&path_env), &dirs), Some(in_path));
    assert_eq!(find_zellij_in(None, None, &dirs), Some(in_second));
    let missing = tmp.path().join("missing");
    assert_eq!(find_zellij_in(Some(&missing), Some(&path_env), &dirs), None);
    assert_eq!(find_zellij_in(None, None, &[tmp.path().join("none")]), None);
}

#[test]
fn default_dirs_order_matches_design() {
    let home = PathBuf::from("h");
    assert_eq!(
        default_dirs(&home),
        vec![
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
            home.join(".cargo").join("bin"),
            home.join(".local").join("bin"),
        ]
    );
}

fn shell(script: &str) -> Command {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/C");
        c
    } else {
        let mut c = Command::new("sh");
        c.arg("-c");
        c
    };
    cmd.arg(script);
    cmd
}

#[test]
fn command_output_is_collected() {
    let out = output_with_timeout(&mut shell("echo hi"), Duration::from_secs(10))
        .unwrap()
        .expect("finished in time");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
}

#[test]
fn hung_command_is_killed_at_timeout() {
    let script = if cfg!(windows) {
        "ping -n 30 127.0.0.1 >NUL"
    } else {
        "sleep 30"
    };
    let start = Instant::now();
    let out = output_with_timeout(&mut shell(script), Duration::from_millis(300)).unwrap();
    assert!(out.is_none());
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
}
```

`crates/mai-probe/tests/spool.rs` 完整内容：

```rust
use std::fs::OpenOptions;
use std::io::Write;

use mai_probe::spool::{DAY_GRACE_MS, Spool, SpoolRecord, make_cursor, sanitize_client};
use serde_json::json;

const DAY: u64 = 86_400_000;
/// A clock far past every test day, so no day file is still in its grace period.
const LATER: u64 = u64::MAX;

fn rec(ts_ms: u64, pane: u32) -> SpoolRecord {
    SpoolRecord {
        ts_ms,
        agent: "claude".into(),
        session: "work".into(),
        pane_id: pane,
        payload: json!({"hook_event_name": "Stop"}),
    }
}

#[test]
fn append_then_read_all_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.append(&rec(10 * DAY + 1, 1)).unwrap();
    s.append(&rec(10 * DAY + 2, 2)).unwrap();
    s.append(&rec(11 * DAY + 1, 3)).unwrap();
    let r = s.read_after(0, LATER).unwrap();
    let panes: Vec<u32> = r.records.iter().map(|(_, x)| x.pane_id).collect();
    assert_eq!(panes, vec![1, 2, 3]);
    assert_eq!(r.bad_lines, 0);
    let cursors: Vec<u64> = r.records.iter().map(|(c, _)| *c).collect();
    assert!(cursors.windows(2).all(|w| w[0] < w[1]), "{cursors:?}");
    assert_eq!(cursors[2] >> 40, 11);
}

#[test]
fn read_after_cursor_skips_seen_records() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.append(&rec(5 * DAY, 1)).unwrap();
    s.append(&rec(5 * DAY, 2)).unwrap();
    let first = s.read_after(0, LATER).unwrap().records[0].0;
    let rest = s.read_after(first, LATER).unwrap();
    assert_eq!(rest.records.len(), 1);
    assert_eq!(rest.records[0].1.pane_id, 2);
    let last = rest.records[0].0;
    assert!(s.read_after(last, LATER).unwrap().records.is_empty());
}

#[test]
fn partial_trailing_line_waits_for_newline() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.append(&rec(7 * DAY, 1)).unwrap();
    let path = dir.path().join("7.jsonl");
    let mut f = OpenOptions::new().append(true).open(&path).unwrap();
    f.write_all(b"{\"ts_ms\":1").unwrap();
    let r = s.read_after(0, LATER).unwrap();
    assert_eq!(r.records.len(), 1);
    assert_eq!(r.bad_lines, 0);
    assert_eq!(
        r.records[0].0,
        make_cursor(7, std::fs::metadata(&path).unwrap().len() - 10)
    );
}

#[test]
fn bad_lines_are_counted_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    std::fs::write(dir.path().join("3.jsonl"), b"not json\n").unwrap();
    s.append(&rec(3 * DAY, 9)).unwrap();
    let r = s.read_after(0, LATER).unwrap();
    assert_eq!(r.bad_lines, 1);
    assert_eq!(r.records.len(), 1);
    assert_eq!(r.end_cursor, r.records[0].0);
    std::fs::write(dir.path().join("4.jsonl"), b"also bad\n").unwrap();
    let r = s.read_after(r.end_cursor, LATER).unwrap();
    assert_eq!((r.records.len(), r.bad_lines), (0, 1));
    let again = s.read_after(r.end_cursor, LATER).unwrap();
    assert_eq!((again.records.len(), again.bad_lines), (0, 0));
    assert_eq!(again.end_cursor, r.end_cursor);
}

#[test]
fn missing_dir_reads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path().join("nope"));
    assert!(s.read_after(0, LATER).unwrap().records.is_empty());
    assert_eq!(s.load_ack(), 0);
}

#[test]
fn ack_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.store_ack(make_cursor(4, 123)).unwrap();
    assert_eq!(s.load_ack(), make_cursor(4, 123));
}

#[test]
fn cleanup_removes_only_old_days() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.append(&rec(DAY, 1)).unwrap();
    s.append(&rec(9 * DAY, 2)).unwrap();
    s.store_ack(1).unwrap();
    let removed = s.cleanup(10 * DAY, 7).unwrap();
    assert_eq!(removed, 1);
    let left: Vec<u32> = s
        .read_after(0, LATER)
        .unwrap()
        .records
        .iter()
        .map(|(_, r)| r.pane_id)
        .collect();
    assert_eq!(left, vec![2]);
    assert_eq!(s.load_ack(), 1);
}

#[test]
fn new_day_file_waits_for_grace_period() {
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::new(dir.path());
    s.append(&rec(20 * DAY + 5, 1)).unwrap();
    let first = s.read_after(0, 20 * DAY + DAY_GRACE_MS).unwrap();
    assert_eq!(first.records.len(), 1);

    // Just after midnight a record lands in the new day's file, and a
    // late hook stamped before midnight lands in the old one.
    s.append(&rec(21 * DAY + 1, 2)).unwrap();
    s.append(&rec(21 * DAY - 1, 3)).unwrap();
    let early = s.read_after(first.end_cursor, 21 * DAY + 100).unwrap();
    let panes: Vec<u32> = early.records.iter().map(|(_, r)| r.pane_id).collect();
    assert_eq!(
        panes,
        vec![3],
        "new day's file is left alone during the grace period"
    );

    let later = s
        .read_after(early.end_cursor, 21 * DAY + DAY_GRACE_MS)
        .unwrap();
    let panes: Vec<u32> = later.records.iter().map(|(_, r)| r.pane_id).collect();
    assert_eq!(panes, vec![2]);
}

#[test]
fn each_client_has_its_own_ack() {
    let dir = tempfile::tempdir().unwrap();
    let a = Spool::for_client(dir.path(), "mac-app");
    let b = Spool::for_client(dir.path(), "win-app");
    a.store_ack(make_cursor(1, 10)).unwrap();
    assert_eq!(a.load_ack(), make_cursor(1, 10));
    assert_eq!(b.load_ack(), 0);
    assert_eq!(Spool::new(dir.path()).load_ack(), 0);
    assert!(dir.path().join("ack-mac-app").is_file());
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn client_ids_are_sanitized_for_file_names() {
    assert_eq!(sanitize_client(r"a/b\c:d e_f-1"), "abcde_f-1");
    let dir = tempfile::tempdir().unwrap();
    let s = Spool::for_client(dir.path(), "../..");
    s.store_ack(7).unwrap();
    assert_eq!(
        Spool::new(dir.path()).load_ack(),
        7,
        "empty id falls back to `ack`"
    );
}
```

`crates/mai-probe/tests/serve.rs` 完整内容：

```rust
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use mai_probe::rules::default_rules;
use mai_probe::serve::{CLEANUP_EVERY_MS, Intervals, Server};
use mai_probe::spool::{Spool, SpoolRecord};
use mai_probe::zellij::{Zellij, ZellijError};
use mai_protocol::{
    AgentEvent, AgentRule, AgentState, AppMsg, EventSource, PaneInfo, ProbeMsg, ScrapeRules,
    SessionInfo, encode_line,
};
use serde_json::json;

#[derive(Default)]
struct FakeState {
    sessions: Vec<SessionInfo>,
    panes: HashMap<String, Vec<PaneInfo>>,
    screens: HashMap<u32, String>,
    fail_panes: bool,
    calls: Vec<String>,
}

#[derive(Clone, Default)]
struct Fake(Rc<RefCell<FakeState>>);

impl Zellij for Fake {
    fn sessions(&self) -> Result<Vec<SessionInfo>, ZellijError> {
        Ok(self.0.borrow().sessions.clone())
    }
    fn panes(&self, session: &str) -> Result<Vec<PaneInfo>, ZellijError> {
        let s = self.0.borrow();
        if s.fail_panes {
            return Err(ZellijError("boom".into()));
        }
        Ok(s.panes.get(session).cloned().unwrap_or_default())
    }
    fn dump_screen(&self, _session: &str, pane_id: u32) -> Result<String, ZellijError> {
        Ok(self
            .0
            .borrow()
            .screens
            .get(&pane_id)
            .cloned()
            .unwrap_or_default())
    }
    fn paste(&self, session: &str, pane_id: u32, text: &str) -> Result<(), ZellijError> {
        self.0
            .borrow_mut()
            .calls
            .push(format!("paste {session} {pane_id} {text}"));
        Ok(())
    }
    fn focus(&self, session: &str, tab_id: u32, pane_id: u32) -> Result<(), ZellijError> {
        self.0
            .borrow_mut()
            .calls
            .push(format!("focus {session} {tab_id} {pane_id}"));
        Ok(())
    }
}

fn pane(id: u32, title: &str) -> PaneInfo {
    PaneInfo {
        id,
        tab_id: 0,
        tab_name: "t".into(),
        title: title.into(),
        command: None,
        exited: false,
    }
}

fn session(name: &str) -> SessionInfo {
    SessionInfo {
        name: name.into(),
        exited: false,
    }
}

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/screens/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap()
}

fn stop_record(ts_ms: u64, pane_id: u32) -> SpoolRecord {
    SpoolRecord {
        ts_ms,
        agent: "claude".into(),
        session: "work".into(),
        pane_id,
        payload: json!({"hook_event_name": "Stop", "last_assistant_message": "all done"}),
    }
}

fn events(msgs: &[ProbeMsg]) -> Vec<&AgentEvent> {
    msgs.iter()
        .filter_map(|m| match m {
            ProbeMsg::AgentEvent(e) => Some(e),
            _ => None,
        })
        .collect()
}

fn error_codes(msgs: &[ProbeMsg]) -> Vec<&str> {
    msgs.iter()
        .filter_map(|m| match m {
            ProbeMsg::Error { code, .. } => Some(code.as_str()),
            _ => None,
        })
        .collect()
}

fn server(fake: &Fake, dir: &std::path::Path) -> Server<Fake> {
    Server::new(Some(fake.clone()), Spool::new(dir), &default_rules()).unwrap()
}

#[test]
fn hook_records_become_events_once() {
    let dir = tempfile::tempdir().unwrap();
    Spool::new(dir.path())
        .append(&stop_record(1_000, 7))
        .unwrap();
    let mut s = server(&Fake::default(), dir.path());
    let out = s.tick(0);
    let ev = events(&out);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].state, AgentState::Done);
    assert_eq!(ev[0].source, EventSource::Hook);
    assert_eq!(ev[0].pane.pane_id, 7);
    assert_eq!(ev[0].message.as_deref(), Some("all done"));
    assert!(ev[0].spool_offset.is_some());
    assert!(events(&s.tick(10)).is_empty());
}

#[test]
fn restart_resumes_after_ack() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path());
    spool.append(&stop_record(1_000, 1)).unwrap();
    let mut first = server(&Fake::default(), dir.path());
    let cursor = events(&first.tick(0))[0].spool_offset.unwrap();
    assert!(
        first
            .handle(AppMsg::Ack {
                spool_offset: cursor
            })
            .is_empty()
    );

    spool.append(&stop_record(2_000, 2)).unwrap();
    let mut second = server(&Fake::default(), dir.path());
    let out = second.tick(0);
    let panes: Vec<u32> = events(&out).iter().map(|e| e.pane.pane_id).collect();
    assert_eq!(panes, vec![2]);
}

#[test]
fn sessions_and_panes_are_sent_only_on_change() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![
            session("work"),
            SessionInfo {
                name: "old".into(),
                exited: true,
            },
        ];
        st.panes.insert("work".into(), vec![pane(1, "bash")]);
        st.panes.insert("old".into(), vec![pane(9, "never polled")]);
    }
    let mut s = server(&fake, dir.path());
    let out = s.tick(0);
    assert!(matches!(&out[0], ProbeMsg::Sessions { sessions } if sessions.len() == 2));
    let panes_msgs: Vec<&str> = out
        .iter()
        .filter_map(|m| match m {
            ProbeMsg::Panes { session, .. } => Some(session.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(panes_msgs, vec!["work"]);

    let out = s.tick(2_000);
    assert!(
        !out.iter()
            .any(|m| matches!(m, ProbeMsg::Sessions { .. } | ProbeMsg::Panes { .. }))
    );

    fake.0
        .borrow_mut()
        .panes
        .insert("work".into(), vec![pane(1, "bash"), pane(2, "vim")]);
    let out = s.tick(4_000);
    assert!(
        out.iter()
            .any(|m| matches!(m, ProbeMsg::Panes { panes, .. } if panes.len() == 2))
    );
}

#[test]
fn scrape_identifies_agent_then_infers_state() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert(
            "work".into(),
            vec![pane(4, "C:\\WINDOWS\\system32\\cmd.exe")],
        );
        st.screens.insert(4, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let ev: Vec<(AgentState, EventSource)> = events(&s.tick(0))
        .iter()
        .map(|e| (e.state, e.source))
        .collect();
    assert_eq!(ev, vec![(AgentState::Unknown, EventSource::Scrape)]);
    assert!(events(&s.tick(3_000)).is_empty());
    let out = s.tick(6_000);
    let ev = events(&out);
    assert_eq!(ev.len(), 1);
    assert_eq!(
        (ev[0].agent.as_str(), ev[0].state),
        ("claude", AgentState::Done)
    );
}

#[test]
fn hook_known_pane_is_not_announced_again() {
    let dir = tempfile::tempdir().unwrap();
    Spool::new(dir.path()).append(&stop_record(1, 4)).unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "x")]);
        st.screens.insert(4, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let ev = events(&s.tick(0))
        .iter()
        .map(|e| (e.state, e.source))
        .collect::<Vec<_>>();
    assert_eq!(ev, vec![(AgentState::Done, EventSource::Hook)]);
}

#[test]
fn vanished_pane_is_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "claude")]);
    }
    let mut s = server(&fake, dir.path());
    assert_eq!(events(&s.tick(0)).len(), 1);
    fake.0.borrow_mut().panes.insert("work".into(), vec![]);
    s.tick(3_000);
    fake.0
        .borrow_mut()
        .panes
        .insert("work".into(), vec![pane(4, "claude")]);
    let ev: Vec<AgentState> = events(&s.tick(6_000)).iter().map(|e| e.state).collect();
    assert_eq!(ev, vec![AgentState::Unknown]);
}

#[test]
fn last_session_gone_is_reported_and_agents_pruned() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "claude")]);
    }
    let mut s = server(&fake, dir.path());
    assert_eq!(events(&s.tick(0)).len(), 1);
    fake.0.borrow_mut().sessions = vec![];
    let out = s.tick(2_000);
    assert!(
        out.iter()
            .any(|m| matches!(m, ProbeMsg::Sessions { sessions } if sessions.is_empty())),
        "{out:?}"
    );
    fake.0.borrow_mut().sessions = vec![session("work")];
    let ev: Vec<(AgentState, EventSource)> = events(&s.tick(4_000))
        .iter()
        .map(|e| (e.state, e.source))
        .collect();
    assert_eq!(ev, vec![(AgentState::Unknown, EventSource::Scrape)]);
}

#[test]
fn scraped_agent_leaving_pane_is_reported_exited_once() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "claude")]);
    }
    let mut s = server(&fake, dir.path());
    assert_eq!(events(&s.tick(0)).len(), 1);
    {
        let mut st = fake.0.borrow_mut();
        st.panes.insert(
            "work".into(),
            vec![pane(4, "C:\\WINDOWS\\system32\\cmd.exe")],
        );
        st.screens.insert(4, "C:\\>".into());
    }
    assert!(
        events(&s.tick(3_000)).is_empty(),
        "one miss is not enough to report Exited"
    );
    let ev: Vec<(u32, AgentState, EventSource)> = events(&s.tick(6_000))
        .iter()
        .map(|e| (e.pane.pane_id, e.state, e.source))
        .collect();
    assert_eq!(ev, vec![(4, AgentState::Exited, EventSource::Scrape)]);
    assert!(events(&s.tick(9_000)).is_empty());
}

#[test]
fn prompt_matching_only_needs_input_rules_keeps_scrape_binding() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert(
            "work".into(),
            vec![pane(4, "C:\\WINDOWS\\system32\\cmd.exe")],
        );
        st.screens.insert(4, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let ev: Vec<AgentState> = events(&s.tick(0)).iter().map(|e| e.state).collect();
    assert_eq!(ev, vec![AgentState::Unknown]);
    // A permission prompt that none of claude's screen patterns match.
    fake.0
        .borrow_mut()
        .screens
        .insert(4, " Do you want to proceed?\n > 1. Yes\n".into());
    let mut seen = Vec::new();
    for t in [3_000, 6_000, 9_000, 12_000] {
        seen.extend(events(&s.tick(t)).iter().map(|e| e.state));
    }
    assert_eq!(seen, vec![AgentState::Working, AgentState::NeedsInput]);
}

#[test]
fn hook_exit_blocks_scrape_rebinding_until_agent_leaves_screen() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path());
    spool.append(&stop_record(1, 5)).unwrap();
    spool
        .append(&SpoolRecord {
            payload: json!({"hook_event_name": "SessionEnd"}),
            ..stop_record(2, 5)
        })
        .unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert(
            "work".into(),
            vec![pane(5, "C:\\WINDOWS\\system32\\cmd.exe")],
        );
        // The agent's last screen is still visible after it exited.
        st.screens.insert(5, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let ev: Vec<AgentState> = events(&s.tick(0)).iter().map(|e| e.state).collect();
    assert_eq!(ev, vec![AgentState::Done, AgentState::Exited]);
    assert!(
        events(&s.tick(3_000)).is_empty(),
        "leftover screen must not rebind"
    );

    fake.0.borrow_mut().screens.insert(5, "C:\\>".into());
    assert!(events(&s.tick(6_000)).is_empty());
    fake.0
        .borrow_mut()
        .screens
        .insert(5, fixture("claude-idle"));
    let ev: Vec<(AgentState, EventSource)> = events(&s.tick(9_000))
        .iter()
        .map(|e| (e.state, e.source))
        .collect();
    assert_eq!(ev, vec![(AgentState::Unknown, EventSource::Scrape)]);
}

#[test]
fn persistent_zellij_error_is_reported_once_per_failure() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    fake.0.borrow_mut().sessions = vec![session("work")];
    fake.0.borrow_mut().fail_panes = true;
    let mut s = server(&fake, dir.path());
    assert_eq!(error_codes(&s.tick(0)), vec!["zellij"]);
    assert!(error_codes(&s.tick(2_000)).is_empty());
    fake.0.borrow_mut().fail_panes = false;
    assert!(error_codes(&s.tick(4_000)).is_empty());
    fake.0.borrow_mut().fail_panes = true;
    assert_eq!(error_codes(&s.tick(6_000)), vec!["zellij"]);
}

#[test]
fn old_spool_files_are_cleaned_up_periodically() {
    const DAY: u64 = 86_400_000;
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path());
    spool.append(&stop_record(DAY, 1)).unwrap();
    let mut s = server(&Fake::default(), dir.path());
    let now = 10 * DAY + 20_000;
    s.tick(now);
    assert!(!dir.path().join("1.jsonl").exists());

    spool.append(&stop_record(2 * DAY, 2)).unwrap();
    s.tick(now + 1_000);
    assert!(dir.path().join("2.jsonl").exists(), "not due yet");
    s.tick(now + CLEANUP_EVERY_MS);
    assert!(!dir.path().join("2.jsonl").exists());
}

#[test]
fn session_end_hook_unbinds_pane() {
    let dir = tempfile::tempdir().unwrap();
    let spool = Spool::new(dir.path());
    spool.append(&stop_record(1, 5)).unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert(
            "work".into(),
            vec![pane(5, "C:\\WINDOWS\\system32\\cmd.exe")],
        );
        st.screens.insert(5, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let ev: Vec<AgentState> = events(&s.tick(0)).iter().map(|e| e.state).collect();
    assert_eq!(ev, vec![AgentState::Done]);

    spool
        .append(&SpoolRecord {
            payload: json!({"hook_event_name": "SessionEnd"}),
            ..stop_record(2, 5)
        })
        .unwrap();
    fake.0.borrow_mut().screens.insert(5, "C:\\>".into());
    let ev: Vec<(AgentState, EventSource)> = events(&s.tick(3_000))
        .iter()
        .map(|e| (e.state, e.source))
        .collect();
    assert_eq!(ev, vec![(AgentState::Exited, EventSource::Hook)]);
    assert!(events(&s.tick(6_000)).is_empty());
    assert!(events(&s.tick(12_000)).is_empty());
}

#[test]
fn focus_and_paste_go_to_zellij() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    let mut s = server(&fake, dir.path());
    assert!(
        s.handle(AppMsg::Focus {
            session: "work".into(),
            pane_id: 14,
            tab_id: 2
        })
        .is_empty()
    );
    assert!(
        s.handle(AppMsg::SendText {
            session: "work".into(),
            pane_id: 14,
            text: "yes".into()
        })
        .is_empty()
    );
    assert_eq!(
        fake.0.borrow().calls,
        vec!["focus work 2 14", "paste work 14 yes"]
    );
}

#[test]
fn bad_line_is_reported_and_next_line_works() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = server(&Fake::default(), dir.path());
    assert_eq!(error_codes(&s.handle_line("garbage")), vec!["bad_message"]);
    assert!(s.handle_line("").is_empty());
    let msg = AppMsg::SetInterval {
        pane_poll_ms: 1,
        scrape_ms: 2,
        metrics_ms: 3,
    };
    assert!(s.handle_line(&encode_line(&msg).unwrap()).is_empty());
    assert_eq!(
        s.intervals(),
        Intervals {
            pane_poll_ms: 250,
            scrape_ms: 250,
            metrics_ms: 250
        }
    );
}

#[test]
fn set_interval_keeps_values_above_minimum() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = server(&Fake::default(), dir.path());
    s.handle(AppMsg::SetInterval {
        pane_poll_ms: 0,
        scrape_ms: 5_000,
        metrics_ms: 250,
    });
    assert_eq!(
        s.intervals(),
        Intervals {
            pane_poll_ms: 250,
            scrape_ms: 5_000,
            metrics_ms: 250
        }
    );
}

#[test]
fn bad_rules_are_rejected_and_old_rules_kept() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "claude")]);
    }
    let mut s = server(&fake, dir.path());
    let bad = ScrapeRules {
        agents: vec![AgentRule {
            name: "x".into(),
            title_patterns: vec!["(".into()],
            screen_patterns: vec![],
            needs_input_patterns: vec![],
            done_patterns: vec![],
            ignore_patterns: vec![],
            stable_ms: 1,
        }],
    };
    assert_eq!(
        error_codes(&s.handle(AppMsg::SetRules { rules: bad })),
        vec!["bad_rules"]
    );
    assert_eq!(events(&s.tick(0))[0].agent, "claude");
}

#[test]
fn set_rules_resets_scrape_history() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![pane(4, "claude")]);
        st.screens.insert(4, "a".into());
    }
    let mut s = server(&fake, dir.path());
    s.tick(0);
    fake.0.borrow_mut().screens.insert(4, "b".into());
    assert!(
        s.handle(AppMsg::SetRules {
            rules: default_rules()
        })
        .is_empty()
    );
    assert!(
        events(&s.tick(3_000)).is_empty(),
        "first dump after reset is a baseline"
    );
    fake.0.borrow_mut().screens.insert(4, "c".into());
    let ev: Vec<AgentState> = events(&s.tick(6_000)).iter().map(|e| e.state).collect();
    assert_eq!(ev, vec![AgentState::Working]);
}

#[test]
fn zellij_errors_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let fake = Fake::default();
    fake.0.borrow_mut().sessions = vec![session("work")];
    fake.0.borrow_mut().fail_panes = true;
    let mut s = server(&fake, dir.path());
    assert_eq!(error_codes(&s.tick(0)), vec!["zellij"]);
}

#[test]
fn without_zellij_only_spool_events_flow() {
    let dir = tempfile::tempdir().unwrap();
    Spool::new(dir.path()).append(&stop_record(1, 3)).unwrap();
    let mut s: Server<Fake> = Server::new(None, Spool::new(dir.path()), &default_rules()).unwrap();
    let out = s.tick(0);
    assert_eq!(out.len(), 1);
    assert_eq!(events(&out).len(), 1);
    let codes = error_codes(&s.handle(AppMsg::Focus {
        session: "w".into(),
        pane_id: 1,
        tab_id: 0,
    }))
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    assert_eq!(codes, vec!["zellij_missing"]);
}

#[test]
fn spool_read_error_is_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("spool");
    std::fs::write(&p, b"x").unwrap();
    let mut s = Server::new(Some(Fake::default()), Spool::new(&p), &default_rules()).unwrap();
    let out1 = s.tick(0);
    let out2 = s.tick(10);
    let codes: Vec<&str> = error_codes(&out1)
        .into_iter()
        .chain(error_codes(&out2))
        .collect();
    let spool_read_count = codes.iter().filter(|c| *c == &"spool_read").count();
    assert_eq!(
        spool_read_count, 1,
        "spool_read error should appear exactly once"
    );
}

#[test]
fn hook_pane_missing_from_poll_is_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    Spool::new(dir.path()).append(&stop_record(1, 5)).unwrap();
    let fake = Fake::default();
    {
        let mut st = fake.0.borrow_mut();
        st.sessions = vec![session("work")];
        st.panes.insert("work".into(), vec![]);
        st.screens.insert(5, fixture("claude-idle"));
    }
    let mut s = server(&fake, dir.path());
    let out1 = s.tick(0);
    assert!(
        !events(&out1).is_empty(),
        "first tick should have hook event"
    );

    fake.0.borrow_mut().panes.insert(
        "work".into(),
        vec![pane(5, "C:\\WINDOWS\\system32\\cmd.exe")],
    );
    let out2 = s.tick(3_000);
    let ev = events(&out2);
    let scrape_unknown = ev.iter().any(|e| {
        e.state == AgentState::Unknown && e.source == EventSource::Scrape && e.pane.pane_id == 5
    });
    assert!(
        scrape_unknown,
        "stale hook entry should be pruned, pane identified afresh by screen"
    );
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-probe --test zellij --test spool --test serve`
Expected: 编译失败（找不到 `output_with_timeout`、`DAY_GRACE_MS`、
`CLEANUP_EVERY_MS`，`read_after` 参数个数不符）。

- [ ] **Step 3: `crates/mai-protocol/src/lib.rs`**

```rust
//! Wire protocol shared by mai-probe and the multi-ai app.
//!
//! Transport is JSON Lines: one JSON object per line, tagged by `type`.

mod codec;
mod messages;
mod types;

pub use codec::{decode_line, encode_line};
pub use messages::{AppMsg, ProbeMsg};
pub use types::{
    AgentEvent, AgentRule, AgentState, EventSource, Metrics, PaneInfo,
    PaneRef, ScrapeRules, SessionInfo,
};

/// Bumped on any incompatible change to message shapes.
pub const PROTOCOL_VERSION: u32 = 1;

/// App installation id as passed to `mai-probe serve --client`: only
/// `[A-Za-z0-9_-]` is kept, so the id is safe in file names and needs no
/// shell quoting.
pub fn sanitize_client(id: &str) -> String {
    id.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}
```

- [ ] **Step 4: `crates/mai-probe/src/zellij.rs`**

```rust
//! zellij CLI access: locating the binary, parsing its output, and the
//! `Zellij` trait that `serve` uses (faked in tests).

use std::ffi::OsStr;
use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use mai_protocol::{PaneInfo, SessionInfo};
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZellijError(pub String);

impl fmt::Display for ZellijError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ZellijError {}

/// Operations `serve` needs from zellij.
pub trait Zellij {
    fn sessions(&self) -> Result<Vec<SessionInfo>, ZellijError>;
    fn panes(&self, session: &str) -> Result<Vec<PaneInfo>, ZellijError>;
    fn dump_screen(&self, session: &str, pane_id: u32) -> Result<String, ZellijError>;
    fn paste(&self, session: &str, pane_id: u32, text: &str) -> Result<(), ZellijError>;
    fn focus(&self, session: &str, tab_id: u32, pane_id: u32) -> Result<(), ZellijError>;
}

/// Parse `zellij list-sessions -n`. Lines look like
/// `work [Created 2days ago] (current)` or
/// `old [Created 1h ago] (EXITED - attach to resurrect)`.
pub fn parse_sessions(text: &str) -> Vec<SessionInfo> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .filter_map(|l| {
            let name = l.split(" [Created").next()?.trim();
            (!name.is_empty()).then(|| SessionInfo {
                name: name.to_owned(),
                exited: l.contains("(EXITED"),
            })
        })
        .collect()
}

/// What zellij prints (and possibly fails with) when no session exists.
const NO_SESSIONS: &str = "No active zellij sessions";

/// Interpret `zellij list-sessions -n` output. "No active sessions" is an
/// empty list whatever the exit status, so the app learns that the last
/// session is gone.
pub fn sessions_from_output(
    success: bool,
    stdout: &str,
    stderr: &str,
) -> Result<Vec<SessionInfo>, ZellijError> {
    if stdout.contains(NO_SESSIONS) || stderr.contains(NO_SESSIONS) {
        Ok(Vec::new())
    } else if success {
        Ok(parse_sessions(stdout))
    } else {
        Err(ZellijError(format!(
            "zellij list-sessions failed: {}",
            stderr.trim()
        )))
    }
}

#[derive(Deserialize)]
struct RawPane {
    id: u32,
    is_plugin: bool,
    #[serde(default)]
    title: String,
    #[serde(default)]
    exited: bool,
    #[serde(default)]
    tab_id: u32,
    #[serde(default)]
    tab_name: String,
    terminal_command: Option<String>,
    /// Foreground command currently running in the pane (zellij >= 0.44
    /// with `list-panes -a`); absent while the shell is idle.
    pane_command: Option<String>,
}

/// Parse `zellij action list-panes -a -j`, keeping terminal panes only.
/// `command` is the running foreground command when known, else the
/// command the pane was started with.
pub fn parse_panes(json: &str) -> Result<Vec<PaneInfo>, serde_json::Error> {
    let raw: Vec<RawPane> = serde_json::from_str(json)?;
    Ok(raw
        .into_iter()
        .filter(|p| !p.is_plugin)
        .map(|p| PaneInfo {
            id: p.id,
            tab_id: p.tab_id,
            tab_name: p.tab_name,
            title: p.title,
            command: p.pane_command.or(p.terminal_command),
            exited: p.exited,
        })
        .collect())
}

/// Parse `zellij --version` output (`zellij 0.44.3`).
pub fn parse_version(text: &str) -> Option<String> {
    text.split_whitespace().nth(1).map(str::to_owned)
}

const EXE_NAMES: &[&str] = &["zellij", "zellij.exe"];

fn exe_in(dir: &Path) -> Option<PathBuf> {
    EXE_NAMES.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// Locate zellij: explicit path, then `PATH`, then common install dirs.
/// Non-interactive SSH sessions often lack Homebrew/cargo dirs in `PATH`.
pub fn find_zellij(
    explicit: Option<&Path>,
    path_env: Option<&OsStr>,
    home: &Path,
) -> Option<PathBuf> {
    find_zellij_in(explicit, path_env, &default_dirs(home))
}

/// Common install dirs checked after `PATH`, in order.
pub fn default_dirs(home: &Path) -> Vec<PathBuf> {
    vec![
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        home.join(".cargo").join("bin"),
        home.join(".local").join("bin"),
    ]
}

/// `find_zellij` with an explicit candidate-dir list (testable without
/// depending on what the host has installed).
pub fn find_zellij_in(
    explicit: Option<&Path>,
    path_env: Option<&OsStr>,
    dirs: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.is_file().then(|| p.to_path_buf());
    }
    let from_path = path_env
        .into_iter()
        .flat_map(std::env::split_paths)
        .find_map(|d| exe_in(&d));
    from_path.or_else(|| dirs.iter().find_map(|d| exe_in(d)))
}

/// Last resort on Unix: ask the user's login shell.
pub fn find_via_login_shell(shell: &OsStr) -> Option<PathBuf> {
    let mut cmd = Command::new(shell);
    cmd.args(["-lc", "command -v zellij"]);
    let out = output_with_timeout(&mut cmd, COMMAND_TIMEOUT).ok()??;
    let path = String::from_utf8(out.stdout).ok()?;
    let path = PathBuf::from(path.trim());
    (out.status.success() && path.is_file()).then_some(path)
}

/// How long one zellij command may run before it is killed. A hung
/// `zellij action` must not stall the whole serve loop.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

/// Windows `CREATE_NO_WINDOW`: a probe started without a console must not
/// open a console window for every command it runs.
#[cfg(windows)]
pub fn no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000)
}

#[cfg(not(windows))]
pub fn no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut buf);
        }
        buf
    })
}

fn wait_until(child: &mut Child, deadline: Instant) -> io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Run `cmd` to completion and collect its output, or kill it after
/// `timeout` and return `Ok(None)`. Pipes are drained on threads so a
/// chatty child cannot block on a full pipe.
pub fn output_with_timeout(cmd: &mut Command, timeout: Duration) -> io::Result<Option<Output>> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let Some(status) = wait_until(&mut child, Instant::now() + timeout)? else {
        let _ = child.kill();
        let _ = child.wait();
        return Ok(None);
    };
    Ok(Some(Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    }))
}

/// `Zellij` backed by the real CLI.
pub struct CliZellij {
    exe: PathBuf,
}

impl CliZellij {
    pub fn new(exe: PathBuf) -> Self {
        Self { exe }
    }

    /// Run zellij; the output is returned whatever the exit status.
    fn run_raw<I, S>(&self, args: I) -> Result<Output, ZellijError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(&self.exe);
        cmd.args(args);
        match output_with_timeout(no_window(&mut cmd), COMMAND_TIMEOUT) {
            Ok(Some(out)) => Ok(out),
            Ok(None) => Err(ZellijError(format!(
                "zellij timed out after {}s",
                COMMAND_TIMEOUT.as_secs()
            ))),
            Err(e) => Err(ZellijError(format!("spawn {}: {e}", self.exe.display()))),
        }
    }

    fn run<I, S>(&self, args: I) -> Result<String, ZellijError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.run_raw(args)?;
        if !out.status.success() {
            return Err(ZellijError(format!(
                "zellij exited with {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn action(&self, session: &str, rest: &[&str]) -> Result<String, ZellijError> {
        let mut args = vec!["--session", session, "action"];
        args.extend_from_slice(rest);
        self.run(args)
    }

    pub fn version(&self) -> Result<String, ZellijError> {
        let out = self.run(["--version"])?;
        parse_version(&out).ok_or_else(|| ZellijError(format!("bad version output: {out}")))
    }
}

fn pane_arg(pane_id: u32) -> String {
    format!("terminal_{pane_id}")
}

impl Zellij for CliZellij {
    fn sessions(&self) -> Result<Vec<SessionInfo>, ZellijError> {
        let out = self.run_raw(["list-sessions", "-n"])?;
        sessions_from_output(
            out.status.success(),
            &String::from_utf8_lossy(&out.stdout),
            &String::from_utf8_lossy(&out.stderr),
        )
    }

    fn panes(&self, session: &str) -> Result<Vec<PaneInfo>, ZellijError> {
        let out = self.action(session, &["list-panes", "-a", "-j"])?;
        parse_panes(&out).map_err(|e| ZellijError(format!("list-panes json: {e}")))
    }

    fn dump_screen(&self, session: &str, pane_id: u32) -> Result<String, ZellijError> {
        self.action(session, &["dump-screen", "-p", &pane_arg(pane_id)])
    }

    fn paste(&self, session: &str, pane_id: u32, text: &str) -> Result<(), ZellijError> {
        self.action(session, &["paste", "-p", &pane_arg(pane_id), text])
            .map(drop)
    }

    fn focus(&self, session: &str, tab_id: u32, pane_id: u32) -> Result<(), ZellijError> {
        self.action(session, &["go-to-tab-by-id", &tab_id.to_string()])?;
        self.action(session, &["focus-pane-id", &pane_arg(pane_id)])
            .map(drop)
    }
}
```

- [ ] **Step 5: `crates/mai-probe/src/spool.rs`**

```rust
//! Spool: append-only event log. Hook processes append records; `serve`
//! reads them back in order and reports them to the app.
//!
//! Files are `<dir>/<day>.jsonl`, where `<day>` is days since the Unix
//! epoch (UTC). A record's cursor is `(day << 40) | end_offset`, where
//! `end_offset` is the byte offset just past the record's newline, so
//! cursors increase strictly across all files.
//!
//! A hook stamps its record just before appending it, so a record stamped
//! at 23:59:59.999 can land in yesterday's file just after midnight. The
//! reader therefore leaves a new day's file alone for `DAY_GRACE_MS`
//! after that day starts; once it moves on to a day, earlier files are
//! never read again.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const DAY_MS: u64 = 86_400_000;
/// How long after midnight (UTC) a new day's file is left unread.
pub const DAY_GRACE_MS: u64 = 10_000;
const OFFSET_BITS: u32 = 40;
const OFFSET_MASK: u64 = (1 << OFFSET_BITS) - 1;

/// One hook invocation as written by `mai-probe hook` / `emit`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpoolRecord {
    pub ts_ms: u64,
    pub agent: String,
    pub session: String,
    pub pane_id: u32,
    pub payload: serde_json::Value,
}

/// Records after a cursor, the number of unparsable lines skipped, and
/// the cursor just past the last complete line read (good or bad).
#[derive(Debug, Default)]
pub struct ReadResult {
    pub records: Vec<(u64, SpoolRecord)>,
    pub bad_lines: usize,
    pub end_cursor: u64,
}

pub fn make_cursor(day: u64, end_offset: u64) -> u64 {
    (day << OFFSET_BITS) | (end_offset & OFFSET_MASK)
}

fn split_cursor(cursor: u64) -> (u64, u64) {
    (cursor >> OFFSET_BITS, cursor & OFFSET_MASK)
}

pub struct Spool {
    dir: PathBuf,
    /// Name of the file holding the acknowledged cursor.
    ack_file: String,
}

pub use mai_protocol::sanitize_client;

impl Spool {
    /// Spool whose acknowledged cursor is stored in `ack`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            ack_file: "ack".to_owned(),
        }
    }

    /// Spool whose acknowledged cursor belongs to one app installation
    /// (`ack-<client>`), so two apps watching the same host do not
    /// consume each other's events on restart.
    pub fn for_client(dir: impl Into<PathBuf>, client: &str) -> Self {
        let client = sanitize_client(client);
        if client.is_empty() {
            return Self::new(dir);
        }
        Self {
            dir: dir.into(),
            ack_file: format!("ack-{client}"),
        }
    }

    fn day_file(&self, day: u64) -> PathBuf {
        self.dir.join(format!("{day}.jsonl"))
    }

    /// Append one record as a single write, so concurrent hook processes
    /// do not interleave within a line.
    pub fn append(&self, rec: &SpoolRecord) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let mut line = serde_json::to_vec(rec)?;
        line.push(b'\n');
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.day_file(rec.ts_ms / DAY_MS))?;
        f.write_all(&line)
    }

    /// Spool days present on disk, ascending.
    fn days(&self) -> io::Result<Vec<u64>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e),
        };
        let mut days = Vec::new();
        for entry in entries {
            let name = entry?.file_name();
            let name = name.to_string_lossy();
            if let Some(day) = name.strip_suffix(".jsonl").and_then(|d| d.parse().ok()) {
                days.push(day);
            }
        }
        days.sort_unstable();
        Ok(days)
    }

    /// All complete records whose cursor is greater than `cursor`.
    /// A trailing line without a newline (still being written) is left
    /// for the next call, and so is any day after the cursor's day that
    /// began less than `DAY_GRACE_MS` before `now_ms`.
    pub fn read_after(&self, cursor: u64, now_ms: u64) -> io::Result<ReadResult> {
        let (cur_day, cur_off) = split_cursor(cursor);
        let mut out = ReadResult {
            end_cursor: cursor,
            ..ReadResult::default()
        };
        for day in self.days()?.into_iter().filter(|d| *d >= cur_day) {
            if day > cur_day && now_ms < (day * DAY_MS).saturating_add(DAY_GRACE_MS) {
                break;
            }
            let start = if day == cur_day { cur_off } else { 0 };
            let mut f = File::open(self.day_file(day))?;
            f.seek(SeekFrom::Start(start))?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            let mut offset = start;
            for chunk in buf.split_inclusive(|b| *b == b'\n') {
                if chunk.last() != Some(&b'\n') {
                    break;
                }
                offset += chunk.len() as u64;
                let at = make_cursor(day, offset);
                match serde_json::from_slice(&chunk[..chunk.len() - 1]) {
                    Ok(rec) => out.records.push((at, rec)),
                    Err(_) => out.bad_lines += 1,
                }
                out.end_cursor = at;
            }
        }
        Ok(out)
    }

    /// Last cursor acknowledged by the app; 0 when none.
    pub fn load_ack(&self) -> u64 {
        fs::read_to_string(self.dir.join(&self.ack_file))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Write the cursor via a temp file named after this process, so two
    /// serves never share a temp file.
    pub fn store_ack(&self, cursor: u64) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let tmp = self
            .dir
            .join(format!("{}.{}.tmp", self.ack_file, std::process::id()));
        fs::write(&tmp, cursor.to_string())?;
        fs::rename(tmp, self.dir.join(&self.ack_file))
    }

    /// Delete day files older than `keep_days` before `now_ms`.
    /// Returns how many files were removed.
    pub fn cleanup(&self, now_ms: u64, keep_days: u64) -> io::Result<usize> {
        let today = now_ms / DAY_MS;
        let mut removed = 0;
        for day in self.days()? {
            if day + keep_days < today {
                fs::remove_file(self.day_file(day))?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

/// Default spool directory under the probe home (`<home>/.mai/spool`).
pub fn spool_dir(mai_home: &Path) -> PathBuf {
    mai_home.join("spool")
}
```

- [ ] **Step 6: `crates/mai-probe/src/scrape.rs`**

```rust
//! Screen-scrape fallback: identifies agent panes and infers agent state
//! from zellij screen dumps.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::{Hash, Hasher};

use mai_protocol::{AgentRule, AgentState, PaneRef, ScrapeRules};
use regex::Regex;

/// A rule pattern that failed to compile, with the agent it belongs to.
#[derive(Debug)]
pub struct RuleError {
    pub agent: String,
    pub pattern: String,
    pub source: regex::Error,
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "agent '{}': bad pattern '{}': {}",
            self.agent, self.pattern, self.source
        )
    }
}

impl std::error::Error for RuleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

struct CompiledRule {
    name: String,
    stable_ms: u64,
    title: Vec<Regex>,
    screen: Vec<Regex>,
    needs_input: Vec<Regex>,
    done: Vec<Regex>,
    ignore: Vec<Regex>,
}

fn compile_all(agent: &str, patterns: &[String]) -> Result<Vec<Regex>, RuleError> {
    patterns
        .iter()
        .map(|p| {
            Regex::new(p).map_err(|source| RuleError {
                agent: agent.to_owned(),
                pattern: p.clone(),
                source,
            })
        })
        .collect()
}

fn any_match(res: &[Regex], text: &str) -> bool {
    res.iter().any(|r| r.is_match(text))
}

impl CompiledRule {
    fn compile(r: &AgentRule) -> Result<Self, RuleError> {
        Ok(Self {
            name: r.name.clone(),
            stable_ms: r.stable_ms,
            title: compile_all(&r.name, &r.title_patterns)?,
            screen: compile_all(&r.name, &r.screen_patterns)?,
            needs_input: compile_all(&r.name, &r.needs_input_patterns)?,
            done: compile_all(&r.name, &r.done_patterns)?,
            ignore: compile_all(&r.name, &r.ignore_patterns)?,
        })
    }

    /// Screen text with every ignore-pattern match removed.
    fn clean<'a>(&self, screen: &'a str) -> Cow<'a, str> {
        let mut out = Cow::Borrowed(screen);
        for re in &self.ignore {
            if re.is_match(&out) {
                out = Cow::Owned(re.replace_all(&out, "").into_owned());
            }
        }
        out
    }
}

/// Rules with all regexes compiled once.
pub struct CompiledRules {
    rules: Vec<CompiledRule>,
}

impl CompiledRules {
    pub fn compile(src: &ScrapeRules) -> Result<Self, RuleError> {
        let rules = src
            .agents
            .iter()
            .map(CompiledRule::compile)
            .collect::<Result<_, _>>()?;
        Ok(Self { rules })
    }

    /// Name of the first agent whose title patterns match the pane title
    /// or command. Cheap: needs no screen dump.
    pub fn identify_by_title(&self, title: &str, command: Option<&str>) -> Option<&str> {
        self.rules
            .iter()
            .find(|r| any_match(&r.title, title) || command.is_some_and(|c| any_match(&r.title, c)))
            .map(|r| r.name.as_str())
    }

    /// Title/command match first; otherwise the first agent whose screen
    /// patterns match the screen.
    pub fn identify(&self, title: &str, command: Option<&str>, screen: &str) -> Option<&str> {
        self.identify_by_title(title, command).or_else(|| {
            self.rules
                .iter()
                .find(|r| any_match(&r.screen, screen))
                .map(|r| r.name.as_str())
        })
    }

    /// True when any of `agent`'s own rules still matches the pane: its
    /// title patterns (title or command), screen patterns, or its
    /// needs-input / done patterns. Prompts such as a plan confirmation
    /// may match none of the screen patterns yet still show the agent.
    pub fn still_matches(
        &self,
        agent: &str,
        title: &str,
        command: Option<&str>,
        screen: &str,
    ) -> bool {
        let Some(r) = self.rule(agent) else {
            return false;
        };
        if any_match(&r.title, title) || command.is_some_and(|c| any_match(&r.title, c)) {
            return true;
        }
        let screen = r.clean(screen);
        any_match(&r.screen, &screen)
            || any_match(&r.needs_input, &screen)
            || any_match(&r.done, &screen)
    }

    fn rule(&self, name: &str) -> Option<&CompiledRule> {
        self.rules.iter().find(|r| r.name == name)
    }
}

struct PaneScrape {
    hash: u64,
    stable_since_ms: u64,
    emitted: Option<AgentState>,
}

/// Per-pane screen history used to infer state changes.
#[derive(Default)]
pub struct ScrapeTracker {
    panes: HashMap<PaneRef, PaneScrape>,
}

fn hash_screen(screen: &str) -> u64 {
    let mut h = DefaultHasher::new();
    screen.hash(&mut h);
    h.finish()
}

impl ScrapeTracker {
    /// Feed one screen dump; returns a state only when it changes.
    pub fn observe(
        &mut self,
        rules: &CompiledRules,
        agent: &str,
        pane: &PaneRef,
        screen: &str,
        now_ms: u64,
    ) -> Option<AgentState> {
        let rule = rules.rule(agent)?;
        let screen = rule.clean(screen);
        let hash = hash_screen(&screen);
        let Some(p) = self.panes.get_mut(pane) else {
            self.panes.insert(
                pane.clone(),
                PaneScrape {
                    hash,
                    stable_since_ms: now_ms,
                    emitted: None,
                },
            );
            return None;
        };
        let next = if p.hash != hash {
            p.hash = hash;
            p.stable_since_ms = now_ms;
            AgentState::Working
        } else if now_ms.saturating_sub(p.stable_since_ms) < rule.stable_ms {
            return None;
        } else if any_match(&rule.needs_input, &screen) {
            AgentState::NeedsInput
        } else if any_match(&rule.done, &screen) {
            AgentState::Done
        } else {
            p.emitted = None;
            return None;
        };
        if p.emitted == Some(next) {
            return None;
        }
        p.emitted = Some(next);
        Some(next)
    }

    /// Drop history for a pane that no longer exists.
    pub fn forget(&mut self, pane: &PaneRef) {
        self.panes.remove(pane);
    }

    /// Drop all history (used when rules are replaced).
    pub fn reset(&mut self) {
        self.panes.clear();
    }
}
```

- [ ] **Step 7: `crates/mai-probe/src/serve.rs`**

```rust
//! `serve` core: turns spool records, zellij state and screen dumps into
//! `ProbeMsg`s, and applies `AppMsg`s. Apart from the `Zellij` trait and
//! the spool directory it does no IO, so tests drive it with a fake zellij.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Display;

use mai_protocol::{
    AgentEvent, AgentState, AppMsg, EventSource, PaneInfo, PaneRef, ProbeMsg, ScrapeRules,
    SessionInfo, decode_line,
};

use crate::hookmap::map_hook;
use crate::scrape::{CompiledRules, RuleError, ScrapeTracker};
use crate::spool::Spool;
use crate::zellij::Zellij;

/// Shortest interval `AppMsg::SetInterval` may set (one run-loop tick).
pub const MIN_INTERVAL_MS: u64 = 250;

/// Spool day files older than this many days are deleted.
pub const SPOOL_KEEP_DAYS: u64 = 7;

/// How often a long-running serve deletes old spool files.
pub const CLEANUP_EVERY_MS: u64 = 3_600_000;

/// A scrape-identified agent is reported `Exited` only after this many
/// consecutive scrapes in which none of its rules match the pane.
pub const MISSES_TO_EXIT: u8 = 2;

/// Polling intervals; changed by `AppMsg::SetInterval`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Intervals {
    pub pane_poll_ms: u64,
    pub scrape_ms: u64,
    pub metrics_ms: u64,
}

impl Default for Intervals {
    fn default() -> Self {
        Self {
            pane_poll_ms: 2_000,
            scrape_ms: 3_000,
            metrics_ms: 2_000,
        }
    }
}

/// `ProbeMsg::Error` with a stable machine-readable `code`.
pub fn error(code: &str, e: impl Display) -> ProbeMsg {
    ProbeMsg::Error {
        code: code.to_owned(),
        message: e.to_string(),
    }
}

/// True when `every_ms` has passed since `last` (or it never ran).
pub(crate) fn due(last: Option<u64>, every_ms: u64, now_ms: u64) -> bool {
    last.is_none_or(|t| now_ms.saturating_sub(t) >= every_ms)
}

/// Agent bound to a pane, and whether a hook (rather than scraping)
/// established the binding. Scrape bindings are re-checked every scrape.
struct Binding {
    agent: String,
    from_hook: bool,
    /// Consecutive scrapes in which none of the agent's rules matched.
    misses: u8,
}

/// What a periodic operation was doing when it failed. A failure is
/// reported once, when it starts; the scope must succeed again before a
/// new failure is reported.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Scope {
    Sessions,
    Panes(String),
    Dump(PaneRef),
    Cleanup,
}

/// Push `msg` unless `scope` is already failing.
fn report(failing: &mut HashSet<Scope>, scope: Scope, msg: ProbeMsg, out: &mut Vec<ProbeMsg>) {
    if failing.insert(scope) {
        out.push(msg);
    }
}

pub struct Server<Z> {
    zellij: Option<Z>,
    spool: Spool,
    rules: CompiledRules,
    scrape: ScrapeTracker,
    intervals: Intervals,
    read_cursor: u64,
    last_pane_poll: Option<u64>,
    last_scrape: Option<u64>,
    last_cleanup: Option<u64>,
    sessions: Vec<SessionInfo>,
    panes: BTreeMap<String, Vec<PaneInfo>>,
    /// Panes known to run an agent (from hooks or identification).
    agents: HashMap<PaneRef, Binding>,
    /// Panes whose agent a hook reported `Exited`. Scraping may bind them
    /// again only after one scrape in which no agent is identified, so a
    /// leftover title or last screen does not bring the agent back.
    exit_hold: HashSet<PaneRef>,
    failing: HashSet<Scope>,
    spool_failing: bool,
}

impl<Z: Zellij> Server<Z> {
    /// `zellij` is `None` when the binary was not found; the server then
    /// only relays spool events. Reading resumes after the last ack.
    pub fn new(zellij: Option<Z>, spool: Spool, rules: &ScrapeRules) -> Result<Self, RuleError> {
        Ok(Self {
            zellij,
            read_cursor: spool.load_ack(),
            spool,
            rules: CompiledRules::compile(rules)?,
            scrape: ScrapeTracker::default(),
            intervals: Intervals::default(),
            last_pane_poll: None,
            last_scrape: None,
            last_cleanup: None,
            sessions: Vec::new(),
            panes: BTreeMap::new(),
            agents: HashMap::new(),
            exit_hold: HashSet::new(),
            failing: HashSet::new(),
            spool_failing: false,
        })
    }

    pub fn intervals(&self) -> Intervals {
        self.intervals
    }

    /// One scheduling step: old spool files are deleted when due, then
    /// new spool records are read, then panes are polled and screens
    /// scraped when their intervals are due.
    pub fn tick(&mut self, now_ms: u64) -> Vec<ProbeMsg> {
        let mut out = Vec::new();
        if due(self.last_cleanup, CLEANUP_EVERY_MS, now_ms) {
            self.last_cleanup = Some(now_ms);
            match self.spool.cleanup(now_ms, SPOOL_KEEP_DAYS) {
                Ok(_) => {
                    self.failing.remove(&Scope::Cleanup);
                }
                Err(e) => report(
                    &mut self.failing,
                    Scope::Cleanup,
                    error("spool_cleanup", e),
                    &mut out,
                ),
            }
        }
        self.drain_spool(now_ms, &mut out);
        if self.zellij.is_some() {
            if due(self.last_pane_poll, self.intervals.pane_poll_ms, now_ms) {
                self.last_pane_poll = Some(now_ms);
                self.poll_panes(&mut out);
            }
            if due(self.last_scrape, self.intervals.scrape_ms, now_ms) {
                self.last_scrape = Some(now_ms);
                self.scrape_panes(now_ms, &mut out);
            }
        }
        out
    }

    fn drain_spool(&mut self, now_ms: u64, out: &mut Vec<ProbeMsg>) {
        let read = match self.spool.read_after(self.read_cursor, now_ms) {
            Ok(r) => r,
            Err(e) => {
                if !self.spool_failing {
                    out.push(error("spool_read", e));
                    self.spool_failing = true;
                }
                return;
            }
        };
        self.spool_failing = false;
        self.read_cursor = read.end_cursor;
        if read.bad_lines > 0 {
            out.push(error(
                "spool_bad_line",
                format!("skipped {} unparsable spool line(s)", read.bad_lines),
            ));
        }
        for (cursor, rec) in read.records {
            let pane = PaneRef {
                session: rec.session,
                pane_id: rec.pane_id,
            };
            let mapped = map_hook(&rec.agent, &rec.payload);
            if self.zellij.is_some() {
                if mapped
                    .as_ref()
                    .is_some_and(|m| m.state == AgentState::Exited)
                {
                    self.agents.remove(&pane);
                    self.scrape.forget(&pane);
                    self.exit_hold.insert(pane.clone());
                } else {
                    let binding = Binding {
                        agent: rec.agent.clone(),
                        from_hook: true,
                        misses: 0,
                    };
                    self.agents.insert(pane.clone(), binding);
                    self.exit_hold.remove(&pane);
                }
            }
            if let Some(m) = mapped {
                out.push(ProbeMsg::AgentEvent(AgentEvent {
                    pane,
                    agent: rec.agent,
                    source: EventSource::Hook,
                    state: m.state,
                    message: m.message,
                    ts_ms: rec.ts_ms,
                    spool_offset: Some(cursor),
                }));
            }
        }
    }

    fn poll_panes(&mut self, out: &mut Vec<ProbeMsg>) {
        let Some(z) = &self.zellij else { return };
        let sessions = match z.sessions() {
            Ok(s) => {
                self.failing.remove(&Scope::Sessions);
                s
            }
            Err(e) => {
                return report(&mut self.failing, Scope::Sessions, error("zellij", e), out);
            }
        };
        if sessions != self.sessions {
            out.push(ProbeMsg::Sessions {
                sessions: sessions.clone(),
            });
            self.sessions = sessions;
        }
        let mut fresh = BTreeMap::new();
        for s in self.sessions.iter().filter(|s| !s.exited) {
            let scope = Scope::Panes(s.name.clone());
            match z.panes(&s.name) {
                Ok(panes) => {
                    self.failing.remove(&scope);
                    if self.panes.get(&s.name) != Some(&panes) {
                        out.push(ProbeMsg::Panes {
                            session: s.name.clone(),
                            panes: panes.clone(),
                        });
                    }
                    fresh.insert(s.name.clone(), panes);
                }
                Err(e) => {
                    report(&mut self.failing, scope, error("zellij", e), out);
                    if let Some(old) = self.panes.get(&s.name) {
                        fresh.insert(s.name.clone(), old.clone());
                    }
                }
            }
        }
        let exists = |p: &PaneRef| {
            fresh
                .get(&p.session)
                .is_some_and(|ps: &Vec<PaneInfo>| ps.iter().any(|q| q.id == p.pane_id))
        };
        for (session, panes) in &self.panes {
            for p in panes {
                let pane = PaneRef {
                    session: session.clone(),
                    pane_id: p.id,
                };
                if !exists(&pane) {
                    self.scrape.forget(&pane);
                }
            }
        }
        self.agents.retain(|k, _| exists(k));
        self.exit_hold.retain(|k| exists(k));
        let live = &self.sessions;
        self.failing.retain(|s| match s {
            Scope::Panes(name) => live.iter().any(|l| !l.exited && &l.name == name),
            Scope::Dump(p) => exists(p),
            Scope::Sessions | Scope::Cleanup => true,
        });
        self.panes = fresh;
    }

    fn scrape_panes(&mut self, now_ms: u64, out: &mut Vec<ProbeMsg>) {
        let Self {
            zellij,
            rules,
            scrape,
            agents,
            panes,
            exit_hold,
            failing,
            ..
        } = self;
        let Some(z) = zellij else { return };
        for (session, list) in panes.iter() {
            for p in list.iter().filter(|p| !p.exited) {
                let pane = PaneRef {
                    session: session.clone(),
                    pane_id: p.id,
                };
                let scope = Scope::Dump(pane.clone());
                let screen = match z.dump_screen(session, p.id) {
                    Ok(s) => {
                        failing.remove(&scope);
                        s
                    }
                    Err(e) => {
                        report(failing, scope, error("zellij", e), out);
                        continue;
                    }
                };
                let command = p.command.as_deref();
                let found = rules.identify(&p.title, command, &screen);
                let agent = match agents.get_mut(&pane) {
                    Some(b) if b.from_hook => b.agent.clone(),
                    Some(b) => {
                        if found == Some(b.agent.as_str())
                            || rules.still_matches(&b.agent, &p.title, command, &screen)
                        {
                            b.misses = 0;
                            b.agent.clone()
                        } else {
                            b.misses += 1;
                            if b.misses < MISSES_TO_EXIT {
                                continue;
                            }
                            // Agent is gone from a scrape-identified pane
                            // (e.g. back at the shell): report it once.
                            out.push(scrape_event(&pane, &b.agent, AgentState::Exited, now_ms));
                            agents.remove(&pane);
                            scrape.forget(&pane);
                            continue;
                        }
                    }
                    None => {
                        let Some(a) = found else {
                            exit_hold.remove(&pane);
                            continue;
                        };
                        if exit_hold.contains(&pane) {
                            continue;
                        }
                        let binding = Binding {
                            agent: a.to_owned(),
                            from_hook: false,
                            misses: 0,
                        };
                        agents.insert(pane.clone(), binding);
                        out.push(scrape_event(&pane, a, AgentState::Unknown, now_ms));
                        a.to_owned()
                    }
                };
                if let Some(state) = scrape.observe(rules, &agent, &pane, &screen, now_ms) {
                    out.push(scrape_event(&pane, &agent, state, now_ms));
                }
            }
        }
    }

    /// Apply one line read from stdin. A bad line is reported, not fatal.
    pub fn handle_line(&mut self, line: &str) -> Vec<ProbeMsg> {
        if line.trim().is_empty() {
            return Vec::new();
        }
        match decode_line::<AppMsg>(line) {
            Ok(msg) => self.handle(msg),
            Err(e) => {
                let head: String = line.chars().take(80).collect();
                vec![error("bad_message", format!("{e}: {head}"))]
            }
        }
    }

    pub fn handle(&mut self, msg: AppMsg) -> Vec<ProbeMsg> {
        match msg {
            AppMsg::Ack { spool_offset } => self
                .spool
                .store_ack(spool_offset)
                .err()
                .map(|e| error("spool_ack", e))
                .into_iter()
                .collect(),
            AppMsg::SendText {
                session,
                pane_id,
                text,
            } => self.with_zellij(|z| z.paste(&session, pane_id, &text)),
            AppMsg::Focus {
                session,
                pane_id,
                tab_id,
            } => self.with_zellij(|z| z.focus(&session, tab_id, pane_id)),
            AppMsg::SetInterval {
                pane_poll_ms,
                scrape_ms,
                metrics_ms,
            } => {
                self.intervals = Intervals {
                    pane_poll_ms: pane_poll_ms.max(MIN_INTERVAL_MS),
                    scrape_ms: scrape_ms.max(MIN_INTERVAL_MS),
                    metrics_ms: metrics_ms.max(MIN_INTERVAL_MS),
                };
                Vec::new()
            }
            AppMsg::SetRules { rules } => match CompiledRules::compile(&rules) {
                Ok(compiled) => {
                    self.rules = compiled;
                    self.scrape.reset();
                    Vec::new()
                }
                Err(e) => vec![error("bad_rules", e)],
            },
        }
    }

    fn with_zellij<E: Display>(&self, f: impl FnOnce(&Z) -> Result<(), E>) -> Vec<ProbeMsg> {
        match &self.zellij {
            None => vec![error("zellij_missing", "zellij not found on this host")],
            Some(z) => f(z).err().map(|e| error("zellij", e)).into_iter().collect(),
        }
    }
}

fn scrape_event(pane: &PaneRef, agent: &str, state: AgentState, now_ms: u64) -> ProbeMsg {
    ProbeMsg::AgentEvent(AgentEvent {
        pane: pane.clone(),
        agent: agent.to_owned(),
        source: EventSource::Scrape,
        state,
        message: None,
        ts_ms: now_ms,
        spool_offset: None,
    })
}
```

`main.rs` 本任务不改：它在启动时的一次清理与 `Server::tick` 的清理并存无害，
Task 2 会删掉前者。

- [ ] **Step 8: 运行测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: 全部通过（`zellij` 11、`spool` 10、`serve` 22 个）；clippy 无警告。

- [ ] **Step 9: 提交**

```bash
git add crates/mai-protocol crates/mai-probe
git commit -m "fix(probe): zellij timeouts, one-shot errors, spool grace and cleanup

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: 探针单实例与数据目录

**Files:**

- Create: `crates/mai-probe/src/instance.rs`、`crates/mai-probe/tests/instance.rs`
- Modify: `crates/mai-probe/src/lib.rs`、`crates/mai-probe/src/main.rs`、
  `crates/mai-probe/src/install.rs`、`crates/mai-probe/tests/cli.rs`

**Interfaces:**

- Consumes: `Spool::for_client`、`spool::sanitize_client`（Task 1）。
- Produces:
  - `instance::data_dir`：二进制位于 `<dir>/bin/` 且 `<dir>` 名字以 `.mai` 开头时
    为 `<dir>`，否则 `MAI_HOME`，再否则 `<home>/.mai`。
  - `instance::pid_file(data, client)`：`serve-<client>.pid`，空 client 为
    `serve.pid`。
  - `stop_recorded(file, wait) -> Option<u32>`、`record_self(file)`、
    `forget_self(file)`。

    ```rust
    pub fn data_dir(exe: Option<&Path>, mai_home: Option<&OsStr>, home: &Path) -> PathBuf
    pub fn pid_file(data: &Path, client: &str) -> PathBuf
    ```

  - CLI：`mai-probe serve [--zellij P] [--rules F] [--client ID]`、
    `mai-probe stop [--client ID]`（输出一行 `{"stopped": <pid 或 null>}`，总是退出 0）。

- [ ] **Step 1: 写测试**

`crates/mai-probe/tests/instance.rs`：

```rust
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
```

`crates/mai-probe/tests/cli.rs` 完整内容（新增 4 个测试：新 serve 替换旧 serve、
不同 client 共存、无 serve 时 stop、已部署探针的数据目录）：

```rust
//! Contract of the `mai-probe` binary as agents and the app call it.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

/// Run the probe with `MAI_HOME=home`, feeding `stdin`. `in_pane` sets
/// the zellij variables of session `work`, pane 5; otherwise they are
/// removed.
fn probe(home: &Path, args: &[&str], stdin: &str, in_pane: bool) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_mai-probe"));
    cmd.args(args)
        .env("MAI_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if in_pane {
        cmd.env("ZELLIJ_SESSION_NAME", "work")
            .env("ZELLIJ_PANE_ID", "5");
    } else {
        cmd.env_remove("ZELLIJ_SESSION_NAME")
            .env_remove("ZELLIJ_PANE_ID");
    }
    let mut child = cmd.spawn().unwrap();
    // The probe may exit before reading stdin (usage errors): ignore EPIPE.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    child.wait_with_output().unwrap()
}

/// All spool lines under `<home>/spool/*.jsonl`.
fn spool_lines(home: &Path) -> Vec<Value> {
    let Ok(entries) = std::fs::read_dir(home.join("spool")) else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    for e in entries {
        let path = e.unwrap().path();
        if path.extension().is_some_and(|x| x == "jsonl") {
            let text = std::fs::read_to_string(path).unwrap();
            lines.extend(text.lines().map(|l| serde_json::from_str(l).unwrap()));
        }
    }
    lines
}

const STOP: &str = r#"{"hook_event_name": "Stop", "last_assistant_message": "ok"}"#;

#[test]
fn hook_appends_one_spool_line_silently() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(home.path(), &["hook", "claude"], STOP, true);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    let lines = spool_lines(home.path());
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["pane_id"], 5);
    assert_eq!(lines[0]["session"], "work");
    assert_eq!(lines[0]["agent"], "claude");
}

#[test]
fn hook_with_invalid_json_exits_zero_without_spool() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(home.path(), &["hook", "claude"], "{ nope", true);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(spool_lines(home.path()).is_empty());
}

#[test]
fn hook_outside_zellij_exits_zero_without_spool() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(home.path(), &["hook", "claude"], STOP, false);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
    assert!(spool_lines(home.path()).is_empty());
}

#[test]
fn hook_usage_error_never_blocks_the_agent() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(home.path(), &["hook", "claude", "--nope"], STOP, true);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty());
}

#[test]
fn serve_with_missing_explicit_zellij_reports_null_path() {
    let home = tempfile::tempdir().unwrap();
    let missing = home.path().join("no-zellij");
    let out = probe(
        home.path(),
        &["serve", "--zellij", missing.to_str().unwrap()],
        "",
        false,
    );
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    let hello: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
    assert_eq!(hello["type"], "hello", "{text}");
    assert_eq!(hello["zellij_path"], Value::Null, "{text}");
}

#[test]
fn emit_rejects_unknown_state() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(
        home.path(),
        &["emit", "--agent", "a", "--state", "bogus"],
        "",
        true,
    );
    assert_eq!(out.status.code(), Some(2));
    assert!(spool_lines(home.path()).is_empty());
}

#[test]
fn emit_outside_zellij_fails() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(
        home.path(),
        &["emit", "--agent", "a", "--state", "done"],
        "",
        false,
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(spool_lines(home.path()).is_empty());
}

/// Start `serve` for `client` with stdin held open, so it keeps running.
fn spawn_serve(home: &Path, client: &str) -> std::process::Child {
    let missing = home.join("no-zellij");
    Command::new(env!("CARGO_BIN_EXE_mai-probe"))
        .args(["serve", "--client", client, "--zellij"])
        .arg(&missing)
        .env("MAI_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

/// Poll `f` every 50 ms for up to 10 s.
fn wait_for(mut f: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if f() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}

fn pid_in(file: &Path) -> Option<u32> {
    std::fs::read_to_string(file).ok()?.trim().parse().ok()
}

#[test]
fn new_serve_replaces_old_one_and_stop_ends_it() {
    let home = tempfile::tempdir().unwrap();
    let pids = home.path().join("serve-t1.pid");
    let mut first = spawn_serve(home.path(), "t1");
    assert!(wait_for(|| pid_in(&pids) == Some(first.id())));

    let mut second = spawn_serve(home.path(), "t1");
    assert!(
        wait_for(|| first.try_wait().unwrap().is_some()),
        "old serve must be stopped"
    );
    assert!(wait_for(|| pid_in(&pids) == Some(second.id())));

    let out = probe(home.path(), &["stop", "--client", "t1"], "", false);
    assert_eq!(out.status.code(), Some(0));
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reply["stopped"], second.id());
    assert!(wait_for(|| second.try_wait().unwrap().is_some()));
}

#[test]
fn serves_of_different_clients_coexist() {
    let home = tempfile::tempdir().unwrap();
    let mut a = spawn_serve(home.path(), "a");
    assert!(wait_for(
        || pid_in(&home.path().join("serve-a.pid")) == Some(a.id())
    ));
    let mut b = spawn_serve(home.path(), "b");
    assert!(wait_for(
        || pid_in(&home.path().join("serve-b.pid")) == Some(b.id())
    ));
    assert!(
        a.try_wait().unwrap().is_none(),
        "other client's serve keeps running"
    );
    drop(a.stdin.take());
    drop(b.stdin.take());
    assert!(wait_for(|| a.try_wait().unwrap().is_some()));
    assert!(wait_for(|| b.try_wait().unwrap().is_some()));
    assert!(
        !home.path().join("serve-a.pid").exists(),
        "pid file removed on exit"
    );
}

#[test]
fn stop_without_running_serve_reports_null() {
    let home = tempfile::tempdir().unwrap();
    let out = probe(home.path(), &["stop"], "", false);
    assert_eq!(out.status.code(), Some(0));
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reply["stopped"], Value::Null);
}

#[test]
fn deployed_probe_keeps_data_next_to_its_bin_dir() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join(".mai").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let exe = bin.join(if cfg!(windows) {
        "mai-probe.exe"
    } else {
        "mai-probe"
    });
    std::fs::copy(env!("CARGO_BIN_EXE_mai-probe"), &exe).unwrap();
    let elsewhere = root.path().join("elsewhere");
    let mut child = Command::new(&exe)
        .args(["hook", "claude"])
        .env("MAI_HOME", &elsewhere)
        .env("ZELLIJ_SESSION_NAME", "work")
        .env("ZELLIJ_PANE_ID", "5")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(STOP.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(spool_lines(&root.path().join(".mai")).len(), 1);
    assert!(
        !elsewhere.exists(),
        "MAI_HOME is ignored for a deployed probe"
    );
}
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-probe --test instance --test cli`
Expected: 编译失败，找不到 `mai_probe::instance`。

- [ ] **Step 3: 写 `crates/mai-probe/src/instance.rs`**

```rust
//! Where the probe keeps its data, and one `serve` per app installation.
//!
//! Hooks run inside the agent's interactive environment and `serve` runs
//! in an SSH exec environment, so environment variables differ between
//! them. Both run the same deployed binary, so the data dir is derived
//! from the binary's location when it lives in a deploy dir.
//!
//! A half-open SSH connection can leave an old `serve` running; on
//! Windows it also locks the binary so it cannot be replaced. Each
//! `serve` records its pid in `serve-<client>.pid`; a new `serve` for the
//! same client (or `mai-probe stop`) terminates the recorded process.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessesToUpdate, System};

use crate::spool::sanitize_client;

/// Data dir: `<dir>` when the binary is `<dir>/bin/<exe>` and `<dir>`'s
/// name starts with `.mai` (a deploy dir such as `~/.mai`); otherwise
/// `$MAI_HOME` if set, else `<home>/.mai`.
pub fn data_dir(exe: Option<&Path>, mai_home: Option<&OsStr>, home: &Path) -> PathBuf {
    let deployed = exe.and_then(Path::parent).and_then(|bin| {
        let dir = bin.parent()?;
        let is_bin = bin.file_name() == Some(OsStr::new("bin"));
        let is_mai = dir
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(".mai"));
        (is_bin && is_mai).then(|| dir.to_path_buf())
    });
    deployed
        .or_else(|| mai_home.map(PathBuf::from))
        .unwrap_or_else(|| home.join(".mai"))
}

/// `serve-<client>.pid` (or `serve.pid` for an empty client id).
pub fn pid_file(data: &Path, client: &str) -> PathBuf {
    let client = sanitize_client(client);
    if client.is_empty() {
        data.join("serve.pid")
    } else {
        data.join(format!("serve-{client}.pid"))
    }
}

fn read_pid(file: &Path) -> Option<u32> {
    fs::read_to_string(file).ok()?.trim().parse().ok()
}

/// Is `pid` a live process whose name starts with `mai-probe`?
fn is_probe(sys: &mut System, pid: Pid) -> bool {
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid)
        .is_some_and(|p| p.name().to_string_lossy().starts_with("mai-probe"))
}

/// Terminate the `serve` recorded in `file`, unless it is this process or
/// the pid now belongs to some other program. Waits up to `wait` for it
/// to exit. Returns the pid that was stopped.
pub fn stop_recorded(file: &Path, wait: Duration) -> Option<u32> {
    let pid = read_pid(file)?;
    if pid == std::process::id() {
        return None;
    }
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    if !is_probe(&mut sys, pid) {
        return None;
    }
    sys.process(pid)?.kill();
    let deadline = Instant::now() + wait;
    while is_probe(&mut sys, pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    Some(pid.as_u32())
}

/// Record this process as the `serve` for its client.
pub fn record_self(file: &Path) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut tmp = file.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, std::process::id().to_string())?;
    fs::rename(tmp, file)
}

/// Remove `file` if it still names this process (a newer serve may have
/// replaced it).
pub fn forget_self(file: &Path) {
    if read_pid(file) == Some(std::process::id()) {
        let _ = fs::remove_file(file);
    }
}
```

- [ ] **Step 4: `crates/mai-probe/src/lib.rs`**

```rust
//! mai-probe library: logic used by the probe binary.

pub mod hookmap;
pub mod install;
pub mod instance;
pub mod metrics;
pub mod rules;
pub mod run;
pub mod scrape;
pub mod serve;
pub mod spool;
pub mod zellij;
```

- [ ] **Step 5: `crates/mai-probe/src/main.rs`**

```rust
//! mai-probe: remote probe for the multi-ai app.
//!
//! `serve` is started by the app over SSH and speaks JSON Lines on
//! stdin/stdout. `hook` and `emit` are called by agents and only append to
//! the spool, so they never slow an agent down.

use std::env;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use mai_probe::install::{
    CLAUDE_EVENTS, CODEX_EVENTS, Outcome, check_probe_exe, hook_command, install_file,
    uninstall_file,
};
use mai_probe::instance::{data_dir, forget_self, pid_file, record_self, stop_recorded};
use mai_probe::rules::{default_rules, parse_rules};
use mai_probe::run::{now_ms, run};
use mai_probe::serve::Server;
use mai_probe::spool::{Spool, SpoolRecord, spool_dir};
use mai_probe::zellij::{CliZellij, find_via_login_shell, find_zellij};
use mai_protocol::{AgentState, PROTOCOL_VERSION, ProbeMsg};
use serde_json::{Value, json};

/// How long a new `serve` (or `stop`) waits for the old one to exit.
const STOP_WAIT: Duration = Duration::from_secs(3);

#[derive(Parser)]
#[command(name = "mai-probe", version, about = "multi-ai remote probe")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Stream agent events and metrics over stdin/stdout (started by the app).
    Serve {
        /// zellij binary; default: PATH, common install dirs, login shell.
        #[arg(long)]
        zellij: Option<PathBuf>,
        /// Scrape rules TOML; default: built-in rules.
        #[arg(long)]
        rules: Option<PathBuf>,
        /// App installation id: one serve and one ack cursor per client.
        #[arg(long, default_value = "")]
        client: String,
    },
    /// Stop the running serve of a client (used before redeploying).
    Stop {
        #[arg(long, default_value = "")]
        client: String,
    },
    /// Called by agent hooks: record one event (JSON payload on stdin).
    Hook { agent: String },
    /// Report a state for any agent (e.g. cmagent) from inside its pane.
    Emit {
        #[arg(long)]
        agent: String,
        /// unknown | working | needs_input | done | exited
        #[arg(long)]
        state: String,
        #[arg(long)]
        msg: Option<String>,
    },
    /// Add our hooks to Claude Code and Codex configs.
    InstallHooks,
    /// Remove our hooks from Claude Code and Codex configs.
    UninstallHooks,
}

fn home_dir() -> PathBuf {
    let (first, second) = if cfg!(windows) {
        ("USERPROFILE", "HOME")
    } else {
        ("HOME", "USERPROFILE")
    };
    env::var_os(first)
        .or_else(|| env::var_os(second))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// Probe data dir; see `instance::data_dir`.
fn data(home: &Path) -> PathBuf {
    let exe = env::current_exe().ok();
    data_dir(exe.as_deref(), env::var_os("MAI_HOME").as_deref(), home)
}

/// Spool record for the zellij pane this process runs in, if any.
fn record_here(agent: &str, payload: Value) -> Option<SpoolRecord> {
    let session = env::var("ZELLIJ_SESSION_NAME").ok()?;
    let pane_id = env::var("ZELLIJ_PANE_ID").ok()?.parse().ok()?;
    Some(SpoolRecord {
        ts_ms: now_ms(),
        agent: agent.to_owned(),
        session,
        pane_id,
        payload,
    })
}

/// Never fails the agent: problems go to stderr, exit code is always 0.
/// Prints nothing on stdout (Claude Code feeds hook stdout to the model).
fn cmd_hook(home: &Path, agent: &str) {
    let mut text = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut text) {
        return eprintln!("mai-probe hook: read stdin: {e}");
    }
    // Some Windows shells prefix piped text with a UTF-8 BOM.
    let payload: Value = match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
        Ok(v) => v,
        Err(e) => return eprintln!("mai-probe hook: payload is not JSON: {e}"),
    };
    let Some(rec) = record_here(agent, payload) else {
        return; // not inside a zellij pane: nothing to track
    };
    if let Err(e) = Spool::new(spool_dir(&data(home))).append(&rec) {
        eprintln!("mai-probe hook: spool: {e}");
    }
}

fn cmd_emit(home: &Path, agent: &str, state: &str, msg: Option<String>) -> ExitCode {
    if serde_json::from_value::<AgentState>(json!(state)).is_err() {
        eprintln!("mai-probe emit: unknown state '{state}'");
        return ExitCode::from(2);
    }
    let Some(rec) = record_here(agent, json!({"state": state, "message": msg})) else {
        eprintln!("mai-probe emit: not inside a zellij pane (ZELLIJ_PANE_ID unset)");
        return ExitCode::FAILURE;
    };
    match Spool::new(spool_dir(&data(home))).append(&rec) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mai-probe emit: spool: {e}");
            ExitCode::FAILURE
        }
    }
}

/// One JSON line per agent on stdout; exit 1 if any agent failed.
fn cmd_hooks(home: &Path, install: bool) -> ExitCode {
    let exe = match env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("mai-probe: cannot locate own executable: {e}");
            return ExitCode::FAILURE;
        }
    };
    let targets = [
        (
            "claude",
            home.join(".claude"),
            "settings.json",
            CLAUDE_EVENTS,
        ),
        ("codex", home.join(".codex"), "hooks.json", CODEX_EVENTS),
    ];
    if install && let Err(e) = check_probe_exe(&exe) {
        for (agent, ..) in targets {
            println!(
                "{}",
                json!({"agent": agent, "outcome": "error", "error": e})
            );
        }
        return ExitCode::FAILURE;
    }
    let mut ok = true;
    for (agent, dir, file, events) in targets {
        let path = dir.join(file);
        let line = if !dir.is_dir() {
            json!({"agent": agent, "outcome": "skipped", "reason": "agent config dir not found"})
        } else {
            let result = if install {
                install_file(&path, events, &hook_command(&exe, agent), now_ms())
            } else {
                uninstall_file(&path, now_ms())
            };
            match result {
                Ok(outcome) => {
                    let mut line = json!({
                        "agent": agent,
                        "outcome": outcome.as_str(),
                        "path": path.display().to_string(),
                    });
                    if install && agent == "codex" && outcome != Outcome::Unchanged {
                        line["note"] = json!(
                            "Codex runs these hooks only after you trust them: run /hooks in Codex"
                        );
                    }
                    line
                }
                Err(e) => {
                    ok = false;
                    json!({"agent": agent, "outcome": "error", "error": e.to_string()})
                }
            }
        };
        println!("{line}");
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Prints `{"stopped": <pid or null>}`; always succeeds.
fn cmd_stop(home: &Path, client: &str) -> ExitCode {
    let stopped = stop_recorded(&pid_file(&data(home), client), STOP_WAIT);
    println!("{}", json!({ "stopped": stopped }));
    ExitCode::SUCCESS
}

fn cmd_serve(
    home: &Path,
    zellij: Option<PathBuf>,
    rules: Option<PathBuf>,
    client: &str,
) -> io::Result<()> {
    let rules = match rules {
        None => default_rules(),
        Some(p) => parse_rules(&std::fs::read_to_string(&p)?).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", p.display()))
        })?,
    };
    let data = data(home);
    let pids = pid_file(&data, client);
    stop_recorded(&pids, STOP_WAIT);
    record_self(&pids)?;
    // An explicit `--zellij` that does not exist is reported as missing,
    // not replaced by whatever the login shell finds.
    let exe = find_zellij(zellij.as_deref(), env::var_os("PATH").as_deref(), home).or_else(|| {
        let shell = env::var_os("SHELL").filter(|_| !cfg!(windows) && zellij.is_none())?;
        find_via_login_shell(&shell)
    });
    let cli = exe.clone().map(CliZellij::new);
    let hello = ProbeMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        probe_version: env!("CARGO_PKG_VERSION").to_owned(),
        os: env::consts::OS.to_owned(),
        arch: env::consts::ARCH.to_owned(),
        zellij_path: exe.map(|p| p.display().to_string()),
        zellij_version: cli.as_ref().and_then(|c| c.version().ok()),
    };
    let spool = Spool::for_client(spool_dir(&data), client);
    let result = Server::new(cli, spool, &rules)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))
        .and_then(|server| {
            run(
                server,
                hello,
                BufReader::new(io::stdin()),
                io::stdout().lock(),
            )
        });
    forget_self(&pids);
    result
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        // Claude Code treats hook exit code 2 as "block the action": a
        // usage error in a hook command must never do that.
        Err(e) if env::args_os().nth(1).is_some_and(|a| a == "hook") => {
            eprintln!("mai-probe hook: {e}");
            return ExitCode::SUCCESS;
        }
        Err(e) => e.exit(),
    };
    let home = home_dir();
    match cli.cmd {
        Cmd::Serve {
            zellij,
            rules,
            client,
        } => match cmd_serve(&home, zellij, rules, &client) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("mai-probe serve: {e}");
                ExitCode::FAILURE
            }
        },
        Cmd::Stop { client } => cmd_stop(&home, &client),
        Cmd::Hook { agent } => {
            cmd_hook(&home, &agent);
            ExitCode::SUCCESS
        }
        Cmd::Emit { agent, state, msg } => cmd_emit(&home, &agent, &state, msg),
        Cmd::InstallHooks => cmd_hooks(&home, true),
        Cmd::UninstallHooks => cmd_hooks(&home, false),
    }
}
```

- [ ] **Step 6: `crates/mai-probe/src/install.rs` 临时文件名带进程号**

`write_doc` 中把

```rust
    tmp.push(".mai-tmp");
```

改为

```rust
    tmp.push(format!(".mai-tmp-{}", std::process::id()));
```

- [ ] **Step 7: 运行测试与 clippy**

Expected: 全部通过（`instance` 5、`cli` 11 个）；clippy 无警告。
`new_serve_replaces_old_one_and_stop_ends_it` 会真实启动并结束 `mai-probe` 进程，
约 1-3 秒。

- [ ] **Step 8: 提交**

```bash
git add crates/mai-probe
git commit -m "feat(probe): one serve per client, stop command, data dir next to binary

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: 部署时的 serve/stop 参数与替换前停止

**Files:**

- Modify: `crates/mai-core/src/deploy.rs`、`crates/mai-core/tests/deploy.rs`、
  `crates/mai-core/examples/deploy_probe.rs`

**Interfaces:**

- Consumes: `mai_protocol::sanitize_client`（Task 1）；探针的 `serve --client`、
  `stop --client`（Task 2）。
- Produces:
  - `DeployOptions.client: String`（默认空）。部署时若远程已有文件且哈希不同，
    先执行 `<probe> stop [--client ID]`（忽略结果）再上传。
  - `Remote::quote_arg(&str) -> String`、`Remote::join_args(&[String]) -> String`
    （只给不是纯 `[A-Za-z0-9_-]` 的词加引号）、
    `Remote::serve_args(client, zellij: Option<&str>) -> String`。
  - `deploy::serve_argv(client, zellij) -> Vec<String>`、
    `deploy::stop_argv(client) -> Vec<String>`、`deploy::stop_args(client) -> String`。
    client 清洗后为空时不带 `--client`（PowerShell 5.1 会丢弃空参数）。

- [ ] **Step 1: 更新测试 `crates/mai-core/tests/deploy.rs`（完整内容）**

```rust
use std::path::PathBuf;

use mai_core::deploy::{
    DeployError, HookResult, Os, ProbeStore, Remote, Shell, hooks_result, normalize_arch,
    parse_hash, parse_hook_lines, parse_uname, sha256_hex, stop_args,
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
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test deploy`
Expected: 编译失败，找不到 `stop_args`、`serve_args`。

- [ ] **Step 3: `crates/mai-core/src/deploy.rs`（完整内容）**

```rust
//! Install mai-probe on a remote host (design 4.2): detect OS, CPU and
//! login shell, compare SHA-256 with the bundled binary, upload over SFTP
//! when different, then run `install-hooks`.

use std::fmt;
use std::path::PathBuf;

use mai_protocol::sanitize_client;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::ssh::auth::Prompter;
use crate::ssh::client::{ExecOutput, SshError, SshSession};

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

/// Detect OS, CPU, shell and home directory of the remote host.
pub async fn detect<P: Prompter>(s: &SshSession<P>) -> Result<Remote, DeployError> {
    let uname = s.exec("uname -sm").await?;
    if uname.success()
        && let Some((os, arch)) = parse_uname(&uname.stdout_str())
    {
        let home = s.exec("printf '%s' \"$HOME\"").await?.stdout_str();
        if home.is_empty() {
            return Err(DeployError::Detect("empty $HOME".into()));
        }
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
                "not Linux/macOS (uname: {:?}) nor Windows (%OS%: {other:?})",
                uname.stdout_str().trim()
            )));
        }
    };
    let (arch_cmd, home_cmd) = match shell {
        Shell::Cmd => ("echo %PROCESSOR_ARCHITECTURE%", "echo %USERPROFILE%"),
        _ => ("$env:PROCESSOR_ARCHITECTURE", "$env:USERPROFILE"),
    };
    // A Unix other than Linux/macOS also lands here (its sh echoes %OS%
    // verbatim); report uname's output so that case is recognisable.
    let raw_arch = s.exec(arch_cmd).await?.stdout_str();
    let arch = normalize_arch(&raw_arch).ok_or_else(|| {
        DeployError::Detect(format!(
            "unknown CPU {:?} (uname: {:?})",
            raw_arch.trim(),
            uname.stdout_str().trim()
        ))
    })?;
    let home = s.exec(home_cmd).await?.stdout_str().trim().to_owned();
    if home.is_empty() {
        return Err(DeployError::Detect("empty %USERPROFILE%".into()));
    }
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

/// Upload `bytes` to `<login dir>/<dir>/bin/<exe>` via SFTP (paths are
/// relative to the login directory, which is home on OpenSSH servers).
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
    if sftp.try_exists(target.clone()).await.map_err(upload_err)? {
        sftp.remove_file(target.clone())
            .await
            .map_err(|e| DeployError::Upload(format!("replace {target} (probe running?): {e}")))?;
    }
    sftp.rename(tmp, target).await.map_err(upload_err)?;
    Ok(())
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

- [ ] **Step 4: `crates/mai-core/examples/deploy_probe.rs`**

`DeployOptions` 字面量加上新字段：

```rust
    let opts = DeployOptions {
        dir,
        install_hooks: false,
        client: String::new(),
    };
```

- [ ] **Step 5: 运行测试与 clippy**

Expected: `deploy` 12 个测试通过；clippy 无警告。

- [ ] **Step 6: 提交**

```bash
git add crates/mai-core
git commit -m "feat(core): serve/stop arguments per shell; stop old probe before replacing it

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: 探针消息链路 `link`

**Files:**

- Create: `crates/mai-core/src/link.rs`、`crates/mai-core/tests/link.rs`
- Modify: `crates/mai-core/Cargo.toml`、`crates/mai-core/src/lib.rs`

**Interfaces:**

- Produces:
  - `link::ProbeIo { reader: LineReader, writer: LineWriter }`，其中
    `LineReader = Box<dyn AsyncBufRead + Send + Unpin>`、
    `LineWriter = Box<dyn AsyncWrite + Send + Unpin>`。
  - `link::HelloInfo`：`protocol_version`、`probe_version`、`os`、`arch`、
    `zellij_path`、`zellij_version`。
  - `link::LinkError`：`Io(String)`、`Closed`、`Silent`、`NoHello(String)`、
    `Protocol { probe: u32, app: u32 }`。
  - `ProbeLink::new(ProbeIo)`、`hello() -> Result<HelloInfo, LinkError>`、
    `recv() -> Result<ProbeMsg, LinkError>`（无法解析的行变成
    `ProbeMsg::Error { code: "bad_probe_line", .. }`）、`send(&AppMsg)`。
  - 常量：`HELLO_TIMEOUT` 30 秒、`SILENCE_TIMEOUT` 20 秒、`MAX_NOISE_LINES` 50。

- [ ] **Step 1: `crates/mai-core/Cargo.toml` 的 `[dev-dependencies]` 加一行**

```toml
tokio = { version = "1", features = ["full", "test-util"] }
```

（`start_paused` 测试需要 `test-util`。）

- [ ] **Step 2: 写失败测试 `crates/mai-core/tests/link.rs`**

```rust
//! `ProbeLink` against an in-memory probe (tokio duplex pipes).

use mai_core::link::{HELLO_TIMEOUT, LinkError, ProbeIo, ProbeLink, SILENCE_TIMEOUT};
use mai_protocol::{AppMsg, PROTOCOL_VERSION, ProbeMsg, decode_line, encode_line};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

/// The probe's ends: write its stdout, read its stdin.
struct FakeProbe {
    out: DuplexStream,
    input: tokio::io::Lines<BufReader<DuplexStream>>,
}

impl FakeProbe {
    async fn say(&mut self, text: &str) {
        self.out.write_all(text.as_bytes()).await.unwrap();
    }

    async fn send(&mut self, msg: &ProbeMsg) {
        self.say(&encode_line(msg).unwrap()).await;
    }

    async fn heard(&mut self) -> AppMsg {
        decode_line(&self.input.next_line().await.unwrap().unwrap()).unwrap()
    }
}

fn pair() -> (ProbeLink, FakeProbe) {
    let (app_read, probe_out) = tokio::io::duplex(64 * 1024);
    let (app_write, probe_in) = tokio::io::duplex(64 * 1024);
    let link = ProbeLink::new(ProbeIo {
        reader: Box::new(BufReader::new(app_read)),
        writer: Box::new(app_write),
    });
    let probe = FakeProbe {
        out: probe_out,
        input: BufReader::new(probe_in).lines(),
    };
    (link, probe)
}

fn hello(version: u32) -> ProbeMsg {
    ProbeMsg::Hello {
        protocol_version: version,
        probe_version: "0.1.0".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        zellij_path: Some("/usr/bin/zellij".into()),
        zellij_version: Some("0.44.3".into()),
    }
}

#[tokio::test]
async fn hello_is_found_after_shell_noise() {
    let (mut link, mut probe) = pair();
    probe.say("Last login: Mon Sep 22\nwelcome!\n").await;
    probe.send(&hello(PROTOCOL_VERSION)).await;
    let info = link.hello().await.unwrap();
    assert_eq!(info.os, "linux");
    assert_eq!(info.zellij_version.as_deref(), Some("0.44.3"));
}

#[tokio::test]
async fn other_protocol_version_is_refused() {
    let (mut link, mut probe) = pair();
    probe.send(&hello(PROTOCOL_VERSION + 1)).await;
    assert_eq!(
        link.hello().await,
        Err(LinkError::Protocol {
            probe: PROTOCOL_VERSION + 1,
            app: PROTOCOL_VERSION
        })
    );
}

#[tokio::test]
async fn first_message_must_be_hello() {
    let (mut link, mut probe) = pair();
    probe.send(&ProbeMsg::Heartbeat { ts_ms: 1 }).await;
    assert!(matches!(link.hello().await, Err(LinkError::NoHello(_))));
}

#[tokio::test]
async fn endless_noise_is_not_a_probe() {
    let (mut link, mut probe) = pair();
    for i in 0..60 {
        probe.say(&format!("noise {i}\n")).await;
    }
    match link.hello().await {
        Err(LinkError::NoHello(m)) => assert!(m.contains("noise 0"), "{m}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn silent_probe_times_out_waiting_for_hello() {
    let (mut link, _probe) = pair();
    let start = tokio::time::Instant::now();
    assert_eq!(link.hello().await, Err(LinkError::Silent));
    assert!(start.elapsed() >= HELLO_TIMEOUT);
}

#[tokio::test(start_paused = true)]
async fn silence_after_hello_is_detected() {
    let (mut link, mut probe) = pair();
    probe.send(&hello(PROTOCOL_VERSION)).await;
    link.hello().await.unwrap();
    let start = tokio::time::Instant::now();
    assert_eq!(link.recv().await, Err(LinkError::Silent));
    assert!(start.elapsed() >= SILENCE_TIMEOUT);
}

#[tokio::test]
async fn bad_line_is_an_error_message_and_eof_closes() {
    let (mut link, mut probe) = pair();
    probe.send(&hello(PROTOCOL_VERSION)).await;
    link.hello().await.unwrap();
    probe.say("{broken\n").await;
    probe.send(&ProbeMsg::Heartbeat { ts_ms: 5 }).await;
    match link.recv().await.unwrap() {
        ProbeMsg::Error { code, .. } => assert_eq!(code, "bad_probe_line"),
        other => panic!("{other:?}"),
    }
    assert_eq!(link.recv().await.unwrap(), ProbeMsg::Heartbeat { ts_ms: 5 });
    drop(probe);
    assert_eq!(link.recv().await, Err(LinkError::Closed));
}

#[tokio::test]
async fn sent_messages_arrive_as_lines() {
    let (mut link, mut probe) = pair();
    link.send(&AppMsg::Ack { spool_offset: 9 }).await.unwrap();
    assert_eq!(probe.heard().await, AppMsg::Ack { spool_offset: 9 });
}
```

- [ ] **Step 3: 运行，确认失败**

Run: `cargo test -p mai-core --test link`
Expected: 编译失败，找不到 `mai_core::link`。

- [ ] **Step 4: 写 `crates/mai-core/src/link.rs`**

```rust
//! The app side of the probe's JSON Lines stream: handshake, receiving
//! `ProbeMsg`s with a silence watchdog, and sending `AppMsg`s. Knows
//! nothing about SSH or processes; any byte stream pair works.

use std::fmt;
use std::time::Duration;

use mai_protocol::{AppMsg, PROTOCOL_VERSION, ProbeMsg, decode_line, encode_line};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, Lines};

/// The probe must say `Hello` within this time after `serve` starts
/// (finding zellij may run a login shell).
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(30);

/// The probe sends a heartbeat every 5 s; this much silence means the
/// stream is dead even if the transport has not noticed.
pub const SILENCE_TIMEOUT: Duration = Duration::from_secs(20);

/// Lines that are not protocol messages (e.g. printed by a shell rc file)
/// tolerated before `Hello`.
pub const MAX_NOISE_LINES: usize = 50;

pub type LineReader = Box<dyn AsyncBufRead + Send + Unpin>;
pub type LineWriter = Box<dyn AsyncWrite + Send + Unpin>;

/// A started `serve`: its stdout to read and stdin to write.
pub struct ProbeIo {
    pub reader: LineReader,
    pub writer: LineWriter,
}

/// What the probe reported about itself in `Hello`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelloInfo {
    pub protocol_version: u32,
    pub probe_version: String,
    pub os: String,
    pub arch: String,
    pub zellij_path: Option<String>,
    pub zellij_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// Reading or writing the stream failed.
    Io(String),
    /// The probe closed its output (process exited or channel closed).
    Closed,
    /// No `Hello` in time, or nothing at all for `SILENCE_TIMEOUT`.
    Silent,
    /// Output before `Hello` was not a protocol stream.
    NoHello(String),
    /// The probe speaks another protocol version.
    Protocol { probe: u32, app: u32 },
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "probe stream: {e}"),
            Self::Closed => f.write_str("probe stream closed"),
            Self::Silent => f.write_str("probe stopped responding"),
            Self::NoHello(m) => write!(f, "probe did not start: {m}"),
            Self::Protocol { probe, app } => {
                write!(f, "probe protocol {probe}, app protocol {app}")
            }
        }
    }
}

impl std::error::Error for LinkError {}

pub struct ProbeLink {
    lines: Lines<LineReader>,
    writer: LineWriter,
}

impl ProbeLink {
    pub fn new(io: ProbeIo) -> Self {
        Self {
            lines: io.reader.lines(),
            writer: io.writer,
        }
    }

    async fn next_line(&mut self, timeout: Duration) -> Result<String, LinkError> {
        match tokio::time::timeout(timeout, self.lines.next_line()).await {
            Err(_) => Err(LinkError::Silent),
            Ok(Err(e)) => Err(LinkError::Io(e.to_string())),
            Ok(Ok(None)) => Err(LinkError::Closed),
            Ok(Ok(Some(line))) => Ok(line),
        }
    }

    /// Wait for `Hello`, skipping up to `MAX_NOISE_LINES` lines that are
    /// not protocol messages, and check the protocol version.
    pub async fn hello(&mut self) -> Result<HelloInfo, LinkError> {
        let deadline = tokio::time::Instant::now() + HELLO_TIMEOUT;
        let mut noise = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            let line = self.next_line(left).await?;
            match decode_line::<ProbeMsg>(&line) {
                Ok(ProbeMsg::Hello {
                    protocol_version,
                    probe_version,
                    os,
                    arch,
                    zellij_path,
                    zellij_version,
                }) => {
                    if protocol_version != PROTOCOL_VERSION {
                        return Err(LinkError::Protocol {
                            probe: protocol_version,
                            app: PROTOCOL_VERSION,
                        });
                    }
                    return Ok(HelloInfo {
                        protocol_version,
                        probe_version,
                        os,
                        arch,
                        zellij_path,
                        zellij_version,
                    });
                }
                Ok(other) => {
                    return Err(LinkError::NoHello(format!(
                        "first message was not hello: {other:?}"
                    )));
                }
                Err(_) => {
                    noise.push(line);
                    if noise.len() > MAX_NOISE_LINES {
                        let head: String = noise[0].chars().take(120).collect();
                        return Err(LinkError::NoHello(format!("unexpected output: {head}")));
                    }
                }
            }
        }
    }

    /// Next message. A line that does not parse is returned as
    /// `ProbeMsg::Error` with code `bad_probe_line`, not as a failure.
    pub async fn recv(&mut self) -> Result<ProbeMsg, LinkError> {
        let line = self.next_line(SILENCE_TIMEOUT).await?;
        Ok(decode_line(&line).unwrap_or_else(|e| {
            let head: String = line.chars().take(120).collect();
            ProbeMsg::Error {
                code: "bad_probe_line".to_owned(),
                message: format!("{e}: {head}"),
            }
        }))
    }

    pub async fn send(&mut self, msg: &AppMsg) -> Result<(), LinkError> {
        let line = encode_line(msg).map_err(|e| LinkError::Io(e.to_string()))?;
        self.writer
            .write_all(line.as_bytes())
            .await
            .map_err(|e| LinkError::Io(e.to_string()))?;
        self.writer
            .flush()
            .await
            .map_err(|e| LinkError::Io(e.to_string()))
    }
}
```

- [ ] **Step 5: `crates/mai-core/src/lib.rs`**

```rust
//! UI-independent core of the multi-ai app.

pub mod deploy;
pub mod link;
pub mod ssh;
pub mod tracker;
```

- [ ] **Step 6: 运行测试与 clippy**

Expected: `link` 8 个测试通过；clippy 无警告。

- [ ] **Step 7: 提交**

```bash
git add Cargo.lock crates/mai-core
git commit -m "feat(core): probe link with handshake and silence detection

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: 每主机任务 `host`

**Files:**

- Create: `crates/mai-core/src/host.rs`、`crates/mai-core/tests/hosts.rs`
- Modify: `crates/mai-core/src/lib.rs`

**Interfaces:**

- Consumes: `link::{ProbeIo, ProbeLink, HelloInfo, LinkError}`（Task 4）、
  `deploy::HookResult`。
- Produces:
  - 类型（完整定义见 Step 3）：

    ```rust
    pub enum HostKind { Local, Ssh { target: String } }
    pub struct HostConfig { pub id: String, pub kind: HostKind, pub zellij: Option<String> }
    pub enum Problem {
        Auth(String), HostKeyRejected, HostKeyChanged { file: PathBuf, line: usize },
        Deploy(String), Config(String), Protocol { probe: u32, app: u32 },
    } // impl Display
    pub enum OpenError { Retry(String), NeedsUser(Problem) }
    pub struct Opened {
        pub io: ProbeIo,
        pub hooks: Result<Vec<HookResult>, String>,
        pub keep: Box<dyn Any + Send>,
    }
    pub trait Connector: Send + Sync + 'static {
        fn open(&self, host: &HostConfig)
            -> impl Future<Output = Result<Opened, OpenError>> + Send;
    }
    pub enum ConnState {
        Connecting, Up,
        Retrying { retry_in: Duration, reason: String },
        NeedsUser(Problem),
    }
    pub enum HostState { Connecting, Online, Degraded, Offline, AuthRequired }
    pub fn host_state(probe: &ConnState, term: Option<&ConnState>) -> HostState
    pub enum HostEvent {
        Probe(ConnState), Hello(HelloInfo),
        Hooks(Result<Vec<HookResult>, String>), Msg(ProbeMsg), Dropped(AppMsg),
    }
    pub enum HostCommand { Retry, Send(AppMsg), Stop }
    pub async fn run_host<C: Connector>(
        cfg: HostConfig,
        connector: Arc<C>,
        events: UnboundedSender<(String, HostEvent)>,
        cmds: UnboundedReceiver<HostCommand>,
    )
    ```

  - `Backoff`（1 秒起加倍、上限 60 秒；`next_delay`、`reset`）、`STABLE_AFTER` 30 秒。
  - `run_host`：每个 hook `AgentEvent`（带 `spool_offset`）交给事件通道后立即回
    `Ack`；探针不在线时收到的 `Send` 以 `HostEvent::Dropped` 报告。

- [ ] **Step 1: 写失败测试 `crates/mai-core/tests/hosts.rs`**

（Task 6 会在此文件末尾追加 manager 的测试。）

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
        self.send(&ProbeMsg::Hello {
            protocol_version: version,
            probe_version: "0.1.0".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            zellij_path: None,
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

/// Replays `steps` in order; hangs once they run out.
struct Scripted {
    steps: Mutex<VecDeque<Step>>,
    opens: AtomicUsize,
    probes: UnboundedSender<FakeProbe>,
}

impl Scripted {
    fn new(steps: Vec<Step>) -> (Arc<Self>, UnboundedReceiver<FakeProbe>) {
        let (probes, rx) = mpsc::unbounded_channel();
        let c = Self {
            steps: Mutex::new(steps.into()),
            opens: AtomicUsize::new(0),
            probes,
        };
        (Arc::new(c), rx)
    }

    fn opens(&self) -> usize {
        self.opens.load(Ordering::SeqCst)
    }
}

impl Connector for Scripted {
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
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test hosts`
Expected: 编译失败，找不到 `mai_core::host`。

- [ ] **Step 3: 写 `crates/mai-core/src/host.rs`**

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
}

/// Connects to a host, deploys the probe and starts `serve`.
pub trait Connector: Send + Sync + 'static {
    fn open(&self, host: &HostConfig) -> impl Future<Output = Result<Opened, OpenError>> + Send;
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
            Problem::Auth(_) | Problem::HostKeyRejected | Problem::HostKeyChanged { .. }
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
                        Err(e) => e.to_string(),
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
                            e.to_string()
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

- [ ] **Step 4: `crates/mai-core/src/lib.rs`**

```rust
//! UI-independent core of the multi-ai app.

pub mod deploy;
pub mod host;
pub mod link;
pub mod ssh;
pub mod tracker;
```

- [ ] **Step 5: 运行测试与 clippy**

Expected: `hosts` 10 个测试通过（暂停时钟下退避等待瞬间完成）；clippy 无警告。

- [ ] **Step 6: 提交**

```bash
git add crates/mai-core
git commit -m "feat(core): per-host task with backoff, retry and acks

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: 汇总 `monitor` 与编排 `manager`

**Files:**

- Create: `crates/mai-core/src/monitor.rs`、`crates/mai-core/src/manager.rs`、
  `crates/mai-core/tests/monitor.rs`
- Modify: `crates/mai-core/src/tracker.rs`、`crates/mai-core/src/lib.rs`、
  `crates/mai-core/tests/hosts.rs`

**Interfaces:**

- Consumes: `host::*`（Task 5）、`link::HelloInfo`、`tracker::*`。
- Produces:
  - `Tracker::records() -> impl Iterator<Item = &AgentRecord>`。
  - `monitor::Update`：`Host { id, state, probe }`、`Hello { id, info }`、
    `Hooks { id, result }`、`Sessions { id, sessions }`、
    `Panes { id, session, panes }`、`Agent(AgentRecord)`、`Alert(Alert)`、
    `Metrics { id, metrics }`、`ProbeError { id, code, message }`、
    `Dropped { id, msg }`。
  - `monitor::HostView { probe, hello, sessions, panes, metrics }`；
    `Monitor::new(TrackerConfig)`、`apply(id, HostEvent, now_ms) -> Vec<Update>`、
    `acknowledge(&AgentKey) -> Option<Update>`、`remove_host(id)`、`host(id)`、`tracker()`。
    pane 从 `Panes` 中消失或 `exited`、session 消失或 `exited` 时，其 agent 判为
    `Exited`（只报告一次）。
  - `HostManager::start(Arc<C>, TrackerConfig) -> (HostManager<C>, UnboundedReceiver<Update>)`、
    `add_host(HostConfig) -> bool`（id 重复为 false）、`remove_host(id) -> bool`、
    `retry(id) -> bool`、`send(id, AppMsg) -> bool`、`acknowledge(AgentKey)`、
    `host_ids() -> Vec<String>`。丢弃 `HostManager` 后所有任务结束，`Update` 流关闭。

- [ ] **Step 1: 写失败测试**

`crates/mai-core/tests/monitor.rs`：

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
            probe: ConnState::Up
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
```

在 `crates/mai-core/tests/hosts.rs` 的 `use mai_core::link::ProbeIo;` 之后加三行：

```rust
use mai_core::manager::HostManager;
use mai_core::monitor::Update;
use mai_core::tracker::{AgentKey, TrackerConfig};
```

并在文件末尾追加：

```rust

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
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test monitor --test hosts`
Expected: 编译失败，找不到 `mai_core::monitor`、`mai_core::manager`。

- [ ] **Step 3: `crates/mai-core/src/tracker.rs` 加 `records`**

在 `pub fn get(...)` 之后加入（不要对该文件运行 rustfmt）：

```rust
    /// Every tracked agent, in no particular order.
    pub fn records(&self) -> impl Iterator<Item = &AgentRecord> {
        self.agents.values()
    }
```

- [ ] **Step 4: 写 `crates/mai-core/src/monitor.rs`**

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
    Host {
        id: String,
        state: HostState,
        probe: ConnState,
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

    pub fn apply(&mut self, id: &str, ev: HostEvent, now_ms: u64) -> Vec<Update> {
        let id = id.to_owned();
        let view = self.hosts.entry(id.clone()).or_default();
        match ev {
            HostEvent::Probe(probe) => {
                view.probe = Some(probe.clone());
                vec![Update::Host {
                    state: host_state(&probe, None),
                    id,
                    probe,
                }]
            }
            HostEvent::Hello(info) => {
                view.hello = Some(info.clone());
                vec![Update::Hello { id, info }]
            }
            HostEvent::Hooks(result) => vec![Update::Hooks { id, result }],
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

- [ ] **Step 5: 写 `crates/mai-core/src/manager.rs`**

```rust
//! Runs one task per host plus one monitor task, and gives the app a
//! small handle to add, remove and command hosts. Every change comes
//! back as an `Update` on the receiver returned by `HostManager::start`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mai_protocol::AppMsg;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::host::{Connector, HostCommand, HostConfig, HostEvent, run_host};
use crate::monitor::{Monitor, Update};
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

/// Handle to the running hosts. Dropping it stops every host task and,
/// once they have ended, the monitor task (the update stream then ends).
pub struct HostManager<C> {
    connector: Arc<C>,
    hosts: HashMap<String, UnboundedSender<HostCommand>>,
    events: UnboundedSender<(String, HostEvent)>,
    ctl: UnboundedSender<Ctl>,
}

async fn run_monitor(
    mut monitor: Monitor,
    mut events: UnboundedReceiver<(String, HostEvent)>,
    mut ctl: UnboundedReceiver<Ctl>,
    updates: UnboundedSender<Update>,
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
                    removed.insert(id);
                    continue;
                }
                Some(Ctl::Acknowledge(key)) => monitor.acknowledge(&key).into_iter().collect(),
            },
            ev = events.recv() => match ev {
                None => return,
                Some((id, _)) if removed.contains(&id) => continue,
                Some((id, ev)) => monitor.apply(&id, ev, now_ms()),
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
        tokio::spawn(run_monitor(Monitor::new(cfg), events_rx, ctl_rx, updates));
        let manager = Self {
            connector,
            hosts: HashMap::new(),
            events,
            ctl,
        };
        (manager, updates_rx)
    }

    /// Start monitoring `cfg`. False if a host with this id exists.
    pub fn add_host(&mut self, cfg: HostConfig) -> bool {
        if self.hosts.contains_key(&cfg.id) {
            return false;
        }
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = self.ctl.send(Ctl::Added(cfg.id.clone()));
        self.hosts.insert(cfg.id.clone(), tx);
        tokio::spawn(run_host(
            cfg,
            self.connector.clone(),
            self.events.clone(),
            rx,
        ));
        true
    }

    /// Stop monitoring host `id` and forget its state.
    pub fn remove_host(&mut self, id: &str) -> bool {
        let Some(tx) = self.hosts.remove(id) else {
            return false;
        };
        let _ = tx.send(HostCommand::Stop);
        let _ = self.ctl.send(Ctl::Removed(id.to_owned()));
        true
    }

    /// Reconnect host `id` now (after a failure that needs the user, or
    /// to skip a backoff wait).
    pub fn retry(&self, id: &str) -> bool {
        self.command(id, HostCommand::Retry)
    }

    /// Send `msg` to host `id`'s probe. If the probe is down, an
    /// `Update::Dropped` reports it.
    pub fn send(&self, id: &str, msg: AppMsg) -> bool {
        self.command(id, HostCommand::Send(msg))
    }

    /// The user has seen this agent's alert.
    pub fn acknowledge(&self, key: AgentKey) {
        let _ = self.ctl.send(Ctl::Acknowledge(key));
    }

    pub fn host_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.hosts.keys().cloned().collect();
        ids.sort();
        ids
    }

    fn command(&self, id: &str, cmd: HostCommand) -> bool {
        self.hosts.get(id).is_some_and(|tx| tx.send(cmd).is_ok())
    }
}
```

- [ ] **Step 6: `crates/mai-core/src/lib.rs`**

```rust
//! UI-independent core of the multi-ai app.

pub mod deploy;
pub mod host;
pub mod link;
pub mod manager;
pub mod monitor;
pub mod ssh;
pub mod tracker;
```

- [ ] **Step 7: 运行测试与 clippy**

Expected: `monitor` 7 个、`hosts` 13 个测试通过；clippy 无警告。

- [ ] **Step 8: 提交**

```bash
git add crates/mai-core
git commit -m "feat(core): monitor aggregates host events; host manager runs the tasks

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: 真实连接器与端到端示例

**Files:**

- Create: `crates/mai-core/src/connect.rs`、`crates/mai-core/tests/connect.rs`、
  `crates/mai-core/examples/monitor.rs`
- Modify: `crates/mai-core/src/lib.rs`

**Interfaces:**

- Consumes: `host::{Connector, HostConfig, HostKind, OpenError, Opened, Problem}`、
  `deploy::{deploy, DeployOptions, ProbeStore, serve_argv, stop_argv, ..}`、
  `ssh::client::connect`、`ssh::config::{parse_config, resolve}`。
- Produces:
  - `ssh_open_error(SshError) -> OpenError`：`Connect`、`Channel` 重试；`Auth`、
    主机密钥拒绝或变化需要用户处理。
  - `deploy_open_error(DeployError) -> OpenError`：`Ssh(e)` 按上一条；其余为
    `Problem::Deploy`。
  - `local_remote(home) -> Option<Remote>`、
    `place_binary(exe, bytes, stop: impl FnOnce()) -> io::Result<bool>`
    （内容相同则不写；替换已有的不同文件前先调用 `stop`）。
  - `SystemConnector::new(opts, prompter, secrets, probes, ssh_config, home,
    default_user, client)`，公开字段 `dir`（默认 `.mai`）、`install_hooks`（默认 true）；
    `gate(id) -> Arc<tokio::sync::Mutex<()>>`（连接与部署期间持有；03c 的终端连接共用）。
    SSH：每次连接重新读取 ssh config；部署后单独执行 `install-hooks`（失败只作为
    `Opened.hooks` 的错误）；`serve` 经 exec 通道启动并请求 `LANG`、`LC_CTYPE`。
    本机：复制探针到 `<home>/<dir>/bin/`、运行 `install-hooks`、以子进程启动
    `serve`（`kill_on_drop`，Windows `CREATE_NO_WINDOW`）。
  - 示例 `monitor <probes-dir> <seconds> <host>...`：`local` 或 ssh 目标；
    部署到 `~/.mai-e2e`（`MAI_E2E_DIR` 可改），不安装 hook，client 为 `e2e`。

- [ ] **Step 1: 写失败测试 `crates/mai-core/tests/connect.rs`**

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
```

- [ ] **Step 2: 运行，确认失败**

Run: `cargo test -p mai-core --test connect`
Expected: 编译失败，找不到 `mai_core::connect`。

- [ ] **Step 3: 写 `crates/mai-core/src/connect.rs`**

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
    DeployError, DeployOptions, HookResult, ProbeStore, Remote, Shell, deploy, hooks_result,
    normalize_arch, serve_argv, sha256_hex, stop_argv,
};
use crate::host::{Connector, HostConfig, HostKind, OpenError, Opened, Problem};
use crate::link::ProbeIo;
use crate::ssh::auth::{Prompter, SecretStore};
use crate::ssh::client::{ConnectOptions, ExecOutput, SshError, connect};
use crate::ssh::config::{HostSpec, parse_config, resolve};

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
        let ch = session
            .open_exec(&remote.invoke(&report.probe_path, &args), &env)
            .await
            .map_err(ssh_open_error)?;
        let (r, w) = tokio::io::split(ch.into_stream());
        Ok(Opened {
            io: ProbeIo {
                reader: Box::new(BufReader::new(r)),
                writer: Box::new(w),
            },
            hooks,
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
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = no_window(&mut cmd)
            .spawn()
            .map_err(|e| OpenError::Retry(format!("start {}: {e}", exe.display())))?;
        let (Some(out), Some(input)) = (child.stdout.take(), child.stdin.take()) else {
            return Err(OpenError::Retry("probe pipes unavailable".into()));
        };
        Ok(Opened {
            io: ProbeIo {
                reader: Box::new(BufReader::new(out)),
                writer: Box::new(input),
            },
            hooks,
            keep: Box::new(child),
        })
    }
}

impl<P: Prompter, S: SecretStore> Connector for SystemConnector<P, S> {
    async fn open(&self, host: &HostConfig) -> Result<Opened, OpenError> {
        match &host.kind {
            HostKind::Local => self.open_local(host).await,
            HostKind::Ssh { target } => self.open_ssh(host, target).await,
        }
    }
}
```

- [ ] **Step 4: `crates/mai-core/src/lib.rs`（最终）**

```rust
//! UI-independent core of the multi-ai app.

pub mod connect;
pub mod deploy;
pub mod host;
pub mod link;
pub mod manager;
pub mod monitor;
pub mod ssh;
pub mod tracker;
```

- [ ] **Step 5: 写 `crates/mai-core/examples/monitor.rs`**

```rust
//! Manual check: monitor hosts with `HostManager` and print every update.
//!
//! cargo run -p mai-core --example monitor -- <probes-dir> <seconds> <host>...
//!
//! A host is `local` (this machine, probe started as a child process) or
//! an ssh target. The probe goes to `~/.mai-e2e` (override with
//! `MAI_E2E_DIR`) and hooks are NOT installed, so the real `~/.mai`,
//! `~/.claude` and `~/.codex` are untouched. The client id is `e2e`.
//! `<probes-dir>` holds `<target>/mai-probe[.exe]`, e.g. the unpacked
//! `probes` CI artifact. See `common/mod.rs` for SSH-related variables.

#[path = "common/mod.rs"]
#[allow(dead_code)]
mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::{MemoryStore, TermPrompter};
use mai_core::connect::SystemConnector;
use mai_core::deploy::ProbeStore;
use mai_core::host::{HostConfig, HostKind};
use mai_core::manager::HostManager;
use mai_core::monitor::Update;
use mai_core::tracker::TrackerConfig;

fn short(u: &Update) -> String {
    let text = match u {
        Update::Panes { id, session, panes } => {
            format!("Panes {{ {id}/{session}: {} pane(s) }}", panes.len())
        }
        Update::Metrics { id, metrics } => {
            format!("Metrics {{ {id}: cpu {:.0}% }}", metrics.cpu_pct)
        }
        other => format!("{other:?}"),
    };
    text.chars().take(200).collect()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (probes, secs, hosts) = match args.as_slice() {
        [p, s, hosts @ ..] if !hosts.is_empty() => (
            PathBuf::from(p),
            s.parse::<u64>().expect("seconds"),
            hosts.to_vec(),
        ),
        _ => {
            eprintln!(
                "usage: monitor <probes-dir> <seconds> <host>...  (host: local | ssh target)"
            );
            std::process::exit(2);
        }
    };
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let home = common::home();
    let mut connector = SystemConnector::new(
        common::options(),
        Arc::new(TermPrompter),
        Arc::new(MemoryStore::default()),
        ProbeStore { dir: probes },
        Some(
            std::env::var_os("MAI_SSH_CONFIG")
                .map_or_else(|| home.join(".ssh").join("config"), PathBuf::from),
        ),
        home,
        user,
        "e2e".to_owned(),
    );
    connector.dir = std::env::var("MAI_E2E_DIR").unwrap_or_else(|_| ".mai-e2e".into());
    connector.install_hooks = false;

    let (mut manager, mut updates) =
        HostManager::start(Arc::new(connector), TrackerConfig::default());
    for h in &hosts {
        let kind = if h == "local" {
            HostKind::Local
        } else {
            HostKind::Ssh { target: h.clone() }
        };
        manager.add_host(HostConfig {
            id: h.clone(),
            kind,
            zellij: None,
        });
    }
    let deadline = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            u = updates.recv() => match u {
                Some(u) => println!("{}", short(&u)),
                None => break,
            },
        }
    }
    drop(manager);
    // Let host tasks drop their connections (the probe exits on EOF).
    tokio::time::sleep(Duration::from_millis(500)).await;
}
```

- [ ] **Step 6: 运行全部测试与 clippy**

Run: `cargo test --workspace --locked`，
`cargo clippy --workspace --all-targets --locked -- -D warnings`
Expected: 190 个测试全部通过（`connect` 4 个）；clippy 无警告。

- [ ] **Step 7: Windows 本机端到端**

```bash
cargo build -p mai-probe --release --locked
mkdir -p probes/x86_64-pc-windows-msvc
cp target/release/mai-probe.exe probes/x86_64-pc-windows-msvc/
cargo run -p mai-core --example monitor -- probes 12 local
```

Expected（顺序）：

```text
Host { id: "local", state: Connecting, .. }
Hello { .. os: "windows" .. }
Hooks { .. Ok([]) }
Host { .. state: Online, probe: Up }
Metrics { local: cpu ..% }      (every 2 s)
```

zellij 不在 PATH 时 `zellij_path: None`，属预期。

清理：确认没有残留进程，删除本机的 `~/.mai-e2e` 与 `probes/`：

```powershell
Get-Process mai-probe -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force -LiteralPath "$env:USERPROFILE\.mai-e2e"
Remove-Item -Recurse -Force -LiteralPath probes
```

- [ ] **Step 8: 提交**

```bash
git add crates/mai-core
git commit -m "feat(core): system connector for SSH and local hosts; monitor example

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: 文档、CI 与远程端到端验证

**Files:**

- Modify: `README.md`、`docs/superpowers/plans/2026-09-24-02-followups.md`

- [ ] **Step 1: 更新 `README.md`**

- Crates 表 `mai-core` 一行改为：
  `UI-independent core: SSH, probe deploy, host manager, agent state tracking`。
- `## mai-core` 小节在 `deploy::deploy` 一条之后加：

```markdown
- `manager::HostManager` runs one task per host (`host::run_host`): connect,
  deploy, start `serve`, relay messages, reconnect with backoff (1 s doubling
  to 60 s). Problems that need the user (authentication, host keys, deploy,
  config, protocol) wait for `retry`. `monitor::Monitor` merges all hosts
  into `Update`s (host state, sessions, panes, agents, alerts, metrics).
- `connect::SystemConnector` reaches SSH hosts (probe on an exec channel) and
  the local machine (probe as a child process, no sshd needed).
```

- Manual checks 代码块加一行：

```bash
cargo run -p mai-core --example monitor -- <probes-dir> <seconds> <host>...  # host: local | ssh target
```

- `## mai-probe` 代码块改为：

```bash
mai-probe serve [--zellij PATH] [--rules FILE] [--client ID]  # JSON Lines on stdin/stdout
mai-probe stop [--client ID]                     # stop that client's running serve
mai-probe hook <agent>                           # called by agent hooks
mai-probe emit --agent A --state S [--msg M]     # report state from any agent
mai-probe install-hooks | uninstall-hooks
```

- 把 `Data dir: ...` 一行替换为：

```markdown
Data dir: `<dir>` when the probe runs from `<dir>/bin/` and `<dir>` starts
with `.mai` (e.g. `~/.mai`); otherwise `$MAI_HOME`, default `~/.mai`
(spool in `spool/`). One `serve` per `--client`: a new one stops the old
one (`serve-<client>.pid`), and each client has its own ack cursor.
```

- [ ] **Step 2: 更新 `docs/superpowers/plans/2026-09-24-02-followups.md`**

- “03-core 必须处理”小节：C1、C2、C3、C4、C6、C7 每条末尾加
  “**（已在 03b 处理）**”，C8 保持原样。
- “03b 必须处理”小节标题改为“03c 必须处理”（该小节剩余条目移交 03c），并：
  - C5 末尾加“**（已在 03b 处理：每主机一个任务串行连接与部署；
    `SystemConnector::gate` 供 03c 终端连接共用）**”。
  - B10 末尾加“**（03b 部分处理：同一应用内已串行；两个应用同时连接同一未知主机
    仍会各自确认）**”。
  - B18 末尾加“**（03b 部分处理：替换前先 `stop` 本 client 的 serve；
    另一个 client 的 serve 仍会锁住 Windows 上的文件）**”。
- “04-app 必须处理”小节：S1、S2、S4 末尾加“**（已在 03b 处理）**”，并新增：
  - **S6 应用重启后 agent 状态丢失**：hook 事件交给监控任务后即确认，spool 不再
    重放；应用重启后，只靠 hook 识别且正停在 `NeedsInput`/`Done` 的 agent 显示为
    未知，直到下一个事件。04 需持久化 `Tracker` 状态，或启动时回放最近的事件。
  - **S7 zellij 查找的登录 shell 若启动常驻子进程**：`output_with_timeout` 在
    进程正常退出后等待管道关闭，若其子进程继承了 stdout 会一直等待；罕见，
    发生时改为超时后放弃读取。
  - **S8 被强制结束的 serve 留下 pid 文件**：无害（下一个 serve 会核对进程），
    可在 04 的“清理”功能中一并删除。

Run: `npx -y markdownlint-cli2 README.md docs/superpowers/specs/*.md docs/superpowers/plans/*.md`
Expected: 0 issues。

- [ ] **Step 3: 提交并推送分支，等待 CI**

```bash
git add README.md docs
git commit -m "docs: host manager, probe single instance and data dir

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
git push -u origin feat/03b-host-manager
gh run watch --repo phalanger/multi-ai --exit-status
```

Expected: CI 三平台测试与 5 个探针目标全部通过。

- [ ] **Step 4: Mac 端到端（用 CI 产出的探针）**

```bash
gh run download <run-id> --repo phalanger/multi-ai --name probes --dir probes
cargo run -p mai-core --example monitor -- probes 25 cyt@100.96.237.7 local
```

Expected：Mac 主机依次出现 `Hello { .. os: "macos", zellij_path: Some(..) .. }`、
`Host { .. Online .. }`、`Sessions`、`Panes`，以及抓屏识别出的 `Agent` 更新；
`local` 同 Task 7。**只读**用户的 zellij session（列 session/pane、抓屏），
不向其发送任何按键或 `SendText`/`Focus`。

清理：

```bash
ssh cyt@100.96.237.7 'pgrep -fl mai-probe || echo none; rm -rf ~/.mai-e2e; ls -d ~/.mai 2>/dev/null || echo "no ~/.mai"'
```

Expected：`none`、`no ~/.mai`（本计划的验证从不创建 `~/.mai`）；本机同 Task 7 清理，
并删除 `probes/`。Ubuntu 主机可选，同样方式验证。

- [ ] **Step 5: 如有修正，提交并推送**

端到端发现的问题修正后重新运行 Task 7 Step 6 与本任务 Step 3。

## 已知限制（留给 03c / 04）

- 终端连接（TermConn：远程 PTY 与本机 portable-pty）与 SSH 加固项（B 类）在 03c。
- 应用重启后 agent 状态需要重新从事件建立（S6）。
- 另一个 client 的 `serve` 运行时，Windows 上无法替换探针（部署报错，等待用户处理）。
- 两个 `serve` 同时启动于同一 client 时（竞态）都会继续运行；同一应用内由每主机
  任务保证不会发生。
- 远程命令只请求 `en_US.UTF-8`；服务器拒绝 env 请求时不回退到命令前缀（设计 7 的
  回退留给 03c 的终端连接一并实现）。
