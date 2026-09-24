# 01 协议、状态机与抓屏分类器 实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task.
> Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 建立 Rust workspace，实现与 spike 结果无关的三块纯逻辑：
`mai-protocol`（消息类型与行编解码）、`mai-core::tracker`（agent 状态合并与提醒决策）、
`mai-probe::scrape`（抓屏识别与状态推断）。

**Architecture:** 三个 crate 都只包含纯逻辑，没有 IO，因此全部可以用单元测试覆盖。
`mai-protocol` 被另外两个 crate 依赖。hook 载荷如何映射成状态、默认抓屏规则写什么，
都依赖 spike 结果，不在本计划范围内（放到 02-probe）。

**Tech Stack:** Rust 1.92（edition 2024）、serde、serde_json、regex。

**Spec:** `docs/superpowers/specs/2026-09-23-multi-ai-monitor-design.md`
（第 2.1、5、6 节）

## Global Constraints

- 源码（代码、字符串、注释）只含 ASCII 字符。
- 零警告：`cargo build` 无警告，
  `cargo clippy --workspace --all-targets -- -D warnings` 通过。
- 协议：JSON Lines，每条消息有 `type` 字段，`PROTOCOL_VERSION = 1`。
- 状态集合：`Unknown`、`Working`、`NeedsInput`、`Done`、`Exited`。
- hook 权威窗口 10 分钟（600000 ms）；提醒去重窗口 30 秒（30000 ms）。
- 抓屏稳定时长由每条规则的 `stable_ms` 指定（设计默认 5000 ms）。
- 修改代码后同步更新 `README.md` 中的开发说明。

---

### Task 1: workspace 与 mai-protocol

**Files:**

- Create: `Cargo.toml`（workspace 根）
- Create: `README.md`
- Create: `crates/mai-protocol/Cargo.toml`
- Create: `crates/mai-protocol/src/lib.rs`
- Create: `crates/mai-protocol/src/types.rs`
- Create: `crates/mai-protocol/src/messages.rs`
- Create: `crates/mai-protocol/src/codec.rs`
- Test: `crates/mai-protocol/tests/roundtrip.rs`

**Interfaces:**

- Produces（后续任务与计划依赖）：
  - `mai_protocol::PROTOCOL_VERSION: u32`
  - `enum AgentState { Unknown, Working, NeedsInput, Done, Exited }`
    （serde 为 snake_case，派生 Copy、Eq、Hash）
  - `enum EventSource { Hook, Scrape }`
  - `struct PaneRef { session: String, pane_id: u32 }`（派生 Eq、Hash）
  - `struct AgentEvent { pane: PaneRef, agent: String, source: EventSource,`
    `state: AgentState, message: Option<String>, ts_ms: u64,`
    `spool_offset: Option<u64> }`
  - `struct SessionInfo { name: String, exited: bool }`
  - `struct PaneInfo { id: u32, tab_id: u32, tab_name: String,`
    `title: String, command: Option<String>, exited: bool }`
  - `struct Metrics { cpu_pct: f32, mem_used: u64, mem_total: u64,`
    `disk_read_bps: u64, disk_write_bps: u64, net_rx_bps: u64,`
    `net_tx_bps: u64, load1: Option<f32>, ts_ms: u64 }`
  - `struct ScrapeRules { agents: Vec<AgentRule> }`
  - `struct AgentRule { name: String, title_patterns: Vec<String>,`
    `screen_patterns: Vec<String>, needs_input_patterns: Vec<String>,`
    `done_patterns: Vec<String>, stable_ms: u64 }`
  - `enum ProbeMsg`（`type` 标签，snake_case）：`Hello`、`Sessions`、
    `Panes`、`AgentEvent(AgentEvent)`、`Metrics(Metrics)`、`Heartbeat`、`Error`
  - `enum AppMsg`：`Ack`、`SendText`、`Focus`、`SetInterval`、`SetRules`
  - `fn encode_line<T: Serialize>(&T) -> Result<String, serde_json::Error>`
  - `fn decode_line<T: DeserializeOwned>(&str) -> Result<T, serde_json::Error>`

- [ ] **Step 1: 创建 workspace 根 `Cargo.toml`**

```toml
[workspace]
resolver = "3"
members = ["crates/mai-protocol"]

[workspace.package]
version = "0.1.0"
edition = "2024"

[workspace.dependencies]
mai-protocol = { path = "crates/mai-protocol" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
regex = "1"

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
```

- [ ] **Step 2: 创建 `crates/mai-protocol/Cargo.toml`**

```toml
[package]
name = "mai-protocol"
version.workspace = true
edition.workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 3: 写失败测试 `crates/mai-protocol/tests/roundtrip.rs`**

```rust
use mai_protocol::{
    AgentEvent, AgentRule, AgentState, AppMsg, EventSource, Metrics,
    PaneInfo, PaneRef, ProbeMsg, ScrapeRules, SessionInfo, decode_line,
    encode_line,
};

fn sample_event() -> AgentEvent {
    AgentEvent {
        pane: PaneRef { session: "work".into(), pane_id: 3 },
        agent: "claude".into(),
        source: EventSource::Hook,
        state: AgentState::NeedsInput,
        message: Some("needs permission".into()),
        ts_ms: 1_700_000_000_000,
        spool_offset: Some(42),
    }
}

fn roundtrip_probe(msg: ProbeMsg) {
    let line = encode_line(&msg).unwrap();
    assert!(line.ends_with('\n'));
    assert_eq!(line.matches('\n').count(), 1, "one line: {line}");
    let back: ProbeMsg = decode_line(&line).unwrap();
    assert_eq!(back, msg);
}

fn roundtrip_app(msg: AppMsg) {
    let line = encode_line(&msg).unwrap();
    assert_eq!(line.matches('\n').count(), 1, "one line: {line}");
    let back: AppMsg = decode_line(&line).unwrap();
    assert_eq!(back, msg);
}

#[test]
fn probe_messages_roundtrip() {
    roundtrip_probe(ProbeMsg::Hello {
        protocol_version: mai_protocol::PROTOCOL_VERSION,
        probe_version: "0.1.0+abc".into(),
        os: "linux".into(),
        arch: "x86_64".into(),
        zellij_path: None,
        zellij_version: Some("0.44.3".into()),
    });
    roundtrip_probe(ProbeMsg::Sessions {
        sessions: vec![SessionInfo { name: "work".into(), exited: false }],
    });
    roundtrip_probe(ProbeMsg::Panes {
        session: "work".into(),
        panes: vec![PaneInfo {
            id: 3,
            tab_id: 1,
            tab_name: "t".into(),
            title: "claude".into(),
            command: None,
            exited: false,
        }],
    });
    roundtrip_probe(ProbeMsg::AgentEvent(sample_event()));
    roundtrip_probe(ProbeMsg::Metrics(Metrics {
        cpu_pct: 12.5,
        mem_used: 1,
        mem_total: 2,
        disk_read_bps: 3,
        disk_write_bps: 4,
        net_rx_bps: 5,
        net_tx_bps: 6,
        load1: None,
        ts_ms: 7,
    }));
    roundtrip_probe(ProbeMsg::Heartbeat { ts_ms: 9 });
    roundtrip_probe(ProbeMsg::Error {
        code: "zellij_missing".into(),
        message: "not found".into(),
    });
}

#[test]
fn app_messages_roundtrip() {
    roundtrip_app(AppMsg::Ack { spool_offset: 42 });
    roundtrip_app(AppMsg::SendText {
        session: "work".into(),
        pane_id: 3,
        text: "line1\nline2".into(),
    });
    roundtrip_app(AppMsg::Focus {
        session: "work".into(),
        pane_id: 3,
        tab_id: 1,
    });
    roundtrip_app(AppMsg::SetInterval {
        pane_poll_ms: 2000,
        scrape_ms: 3000,
        metrics_ms: 2000,
    });
    roundtrip_app(AppMsg::SetRules {
        rules: ScrapeRules {
            agents: vec![AgentRule {
                name: "fake".into(),
                title_patterns: vec!["^fake".into()],
                screen_patterns: vec![],
                needs_input_patterns: vec!["y/n".into()],
                done_patterns: vec![],
                stable_ms: 5000,
            }],
        },
    });
}

#[test]
fn agent_event_is_flat_with_type_tag() {
    let line = encode_line(&ProbeMsg::AgentEvent(sample_event())).unwrap();
    let v: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(v["type"], "agent_event");
    assert_eq!(v["state"], "needs_input");
    assert_eq!(v["source"], "hook");
    assert_eq!(v["pane"]["pane_id"], 3);
}

#[test]
fn optional_fields_may_be_absent() {
    let line = r#"{"type":"agent_event","pane":{"session":"s","pane_id":1},
        "agent":"codex","source":"scrape","state":"done","ts_ms":5}"#
        .replace('\n', "");
    let msg: ProbeMsg = decode_line(&line).unwrap();
    let ProbeMsg::AgentEvent(ev) = msg else {
        panic!("wrong variant")
    };
    assert_eq!(ev.message, None);
    assert_eq!(ev.spool_offset, None);
}

#[test]
fn decode_accepts_crlf() {
    let msg: ProbeMsg =
        decode_line("{\"type\":\"heartbeat\",\"ts_ms\":1}\r\n").unwrap();
    assert_eq!(msg, ProbeMsg::Heartbeat { ts_ms: 1 });
}

#[test]
fn unknown_type_is_error() {
    let r: Result<ProbeMsg, _> = decode_line("{\"type\":\"nope\"}");
    assert!(r.is_err());
}
```

- [ ] **Step 4: 运行测试，确认失败**

Run: `cargo test -p mai-protocol`
Expected: 编译失败，提示找不到 `mai_protocol::AgentEvent` 等符号（此时 `lib.rs` 还不存在）。

- [ ] **Step 5: 写 `crates/mai-protocol/src/lib.rs`**

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
```

- [ ] **Step 6: 写 `crates/mai-protocol/src/types.rs`**

```rust
//! Value types carried inside protocol messages.

use serde::{Deserialize, Serialize};

/// Lifecycle state of an agent running in a zellij pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Unknown,
    Working,
    NeedsInput,
    Done,
    Exited,
}

/// Where an agent event came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    Hook,
    Scrape,
}

/// A pane inside a zellij session on one host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneRef {
    pub session: String,
    pub pane_id: u32,
}

/// A state observation for one agent pane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
    pub pane: PaneRef,
    pub agent: String,
    pub source: EventSource,
    pub state: AgentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub ts_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spool_offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub name: String,
    pub exited: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: u32,
    pub tab_id: u32,
    pub tab_name: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub exited: bool,
}

/// Host metrics sample. Rates are bytes per second.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub cpu_pct: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
    pub net_rx_bps: u64,
    pub net_tx_bps: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load1: Option<f32>,
    pub ts_ms: u64,
}

/// Screen-scrape rules pushed from the app to the probe.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScrapeRules {
    #[serde(default)]
    pub agents: Vec<AgentRule>,
}

/// Regex rules for one agent kind. All patterns use `regex` crate syntax.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentRule {
    pub name: String,
    /// Matched against pane title and pane command to identify the agent.
    #[serde(default)]
    pub title_patterns: Vec<String>,
    /// Matched against screen content to identify the agent.
    #[serde(default)]
    pub screen_patterns: Vec<String>,
    #[serde(default)]
    pub needs_input_patterns: Vec<String>,
    #[serde(default)]
    pub done_patterns: Vec<String>,
    /// Screen must be unchanged this long before idle states are inferred.
    pub stable_ms: u64,
}
```

- [ ] **Step 7: 写 `crates/mai-protocol/src/messages.rs`**

```rust
//! Top-level messages in each direction.

use serde::{Deserialize, Serialize};

use crate::types::{
    AgentEvent, Metrics, PaneInfo, ScrapeRules, SessionInfo,
};

/// Probe -> app (probe stdout).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProbeMsg {
    Hello {
        protocol_version: u32,
        probe_version: String,
        os: String,
        arch: String,
        zellij_path: Option<String>,
        zellij_version: Option<String>,
    },
    Sessions {
        sessions: Vec<SessionInfo>,
    },
    Panes {
        session: String,
        panes: Vec<PaneInfo>,
    },
    AgentEvent(AgentEvent),
    Metrics(Metrics),
    Heartbeat {
        ts_ms: u64,
    },
    Error {
        code: String,
        message: String,
    },
}

/// App -> probe (probe stdin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppMsg {
    Ack {
        spool_offset: u64,
    },
    SendText {
        session: String,
        pane_id: u32,
        text: String,
    },
    Focus {
        session: String,
        pane_id: u32,
        tab_id: u32,
    },
    SetInterval {
        pane_poll_ms: u64,
        scrape_ms: u64,
        metrics_ms: u64,
    },
    SetRules {
        rules: ScrapeRules,
    },
}
```

- [ ] **Step 8: 写 `crates/mai-protocol/src/codec.rs`**

```rust
//! JSON Lines encoding helpers.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Serialize `msg` as one JSON line terminated by `\n`.
///
/// serde_json escapes newlines inside strings, so the output always
/// contains exactly one `\n`.
pub fn encode_line<T: Serialize>(msg: &T) -> Result<String, serde_json::Error> {
    let mut s = serde_json::to_string(msg)?;
    s.push('\n');
    Ok(s)
}

/// Parse one line; a trailing `\n` or `\r\n` is ignored.
pub fn decode_line<T: DeserializeOwned>(
    line: &str,
) -> Result<T, serde_json::Error> {
    serde_json::from_str(line.trim_end_matches(['\r', '\n']))
}
```

- [ ] **Step 9: 运行测试，确认通过**

Run: `cargo test -p mai-protocol`
Expected: 6 个测试全部 PASS。

- [ ] **Step 10: clippy 零警告**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 无警告、无错误。

- [ ] **Step 11: 写 `README.md`**

````markdown
# multi-ai

Monitor AI coding agents (Claude Code, Codex, cmagent, ...) running in
zellij sessions across SSH hosts.

Design: `docs/superpowers/specs/2026-09-23-multi-ai-monitor-design.md`

## Crates

| crate | purpose |
| --- | --- |
| `mai-protocol` | probe/app wire messages (JSON Lines) |

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
````

- [ ] **Step 12: 提交**

```bash
git add Cargo.toml Cargo.lock README.md crates/mai-protocol
git commit -m "feat(protocol): add mai-protocol message types and codec"
```

---

### Task 2: mai-core 状态合并与提醒决策（Tracker）

**Files:**

- Modify: `Cargo.toml`（members 加入 `crates/mai-core`）
- Modify: `README.md`（Crates 表加一行）
- Create: `crates/mai-core/Cargo.toml`
- Create: `crates/mai-core/src/lib.rs`
- Create: `crates/mai-core/src/tracker.rs`
- Test: `crates/mai-core/tests/tracker.rs`

**Interfaces:**

- Consumes: `mai_protocol::{AgentEvent, AgentState, EventSource}`
- Produces:
  - `struct AgentKey { host_id: String, session: String, pane_id: u32 }`
  - `struct TrackerConfig { hook_authority_ms: u64, alert_dedupe_ms: u64 }`，
    `Default` 为 600000 / 30000
  - `struct Alert { key: AgentKey, agent: String, state: AgentState,`
    `message: Option<String> }`
  - `struct AgentRecord`，公开字段：`key`、`agent`、`state`、`message`、
    `acknowledged: bool`、`since_ms: u64`
  - `Tracker::new(TrackerConfig) -> Tracker`
  - `Tracker::apply(&mut self, host_id: &str, ev: &AgentEvent) -> Option<Alert>`
  - `Tracker::pane_gone(&mut self, key: &AgentKey, now_ms: u64)`
  - `Tracker::acknowledge(&mut self, key: &AgentKey) -> bool`
  - `Tracker::get(&self, key: &AgentKey) -> Option<&AgentRecord>`
  - `Tracker::pending(&self) -> Vec<&AgentRecord>`
    （未确认的 NeedsInput 在前、Done 在后，同级按 `since_ms` 升序）

**规则（来自设计第 6.3、6.4 节）：**

1. hook 事件总是被接受，并刷新该 agent 的 `last_hook_ms`。
2. 抓屏事件：若 `ev.ts_ms - last_hook_ms < hook_authority_ms`，只接受
   `Done/NeedsInput -> Working` 这一种转换，其他一律忽略。
3. 状态没有变化：什么都不做。
4. 进入 `NeedsInput` 或 `Done`：设 `acknowledged = false`；若同一状态的
   上次提醒距今不足 `alert_dedupe_ms`，不返回 Alert，否则返回 Alert。
5. 进入其他状态：设 `acknowledged = true`，不返回 Alert。

- [ ] **Step 1: 修改 workspace 根 `Cargo.toml`**

`members` 改为：

```toml
members = ["crates/mai-protocol", "crates/mai-core"]
```

- [ ] **Step 2: 创建 `crates/mai-core/Cargo.toml`**

```toml
[package]
name = "mai-core"
version.workspace = true
edition.workspace = true

[dependencies]
mai-protocol = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 3: 写失败测试 `crates/mai-core/tests/tracker.rs`**

```rust
use mai_core::tracker::{AgentKey, Tracker, TrackerConfig};
use mai_protocol::{AgentEvent, AgentState, EventSource, PaneRef};

use AgentState::{Done, Exited, NeedsInput, Working};
use EventSource::{Hook, Scrape};

const HOST: &str = "host-a";

fn ev(
    pane: u32,
    source: EventSource,
    state: AgentState,
    ts: u64,
) -> AgentEvent {
    AgentEvent {
        pane: PaneRef { session: "work".into(), pane_id: pane },
        agent: "claude".into(),
        source,
        state,
        message: None,
        ts_ms: ts,
        spool_offset: None,
    }
}

fn key(pane: u32) -> AgentKey {
    AgentKey { host_id: HOST.into(), session: "work".into(), pane_id: pane }
}

struct Case {
    name: &'static str,
    steps: Vec<(EventSource, AgentState, u64, Option<AgentState>)>,
    final_state: AgentState,
}

#[test]
fn table() {
    let cases = vec![
        Case {
            name: "hook done alerts",
            steps: vec![(Hook, Working, 0, None), (Hook, Done, 1_000, Some(Done))],
            final_state: Done,
        },
        Case {
            name: "hook needs input alerts",
            steps: vec![
                (Hook, Working, 0, None),
                (Hook, NeedsInput, 1_000, Some(NeedsInput)),
            ],
            final_state: NeedsInput,
        },
        Case {
            name: "same state repeated does not alert",
            steps: vec![(Hook, Done, 0, Some(Done)), (Hook, Done, 1_000, None)],
            final_state: Done,
        },
        Case {
            name: "dedupe within window",
            steps: vec![
                (Hook, Done, 0, Some(Done)),
                (Hook, Working, 1_000, None),
                (Hook, Done, 2_000, None),
            ],
            final_state: Done,
        },
        Case {
            name: "alerts again after dedupe window",
            steps: vec![
                (Hook, Done, 0, Some(Done)),
                (Hook, Working, 1_000, None),
                (Hook, Done, 31_000, Some(Done)),
            ],
            final_state: Done,
        },
        Case {
            name: "scrape ignored while hook authoritative",
            steps: vec![(Hook, Working, 0, None), (Scrape, Done, 1_000, None)],
            final_state: Working,
        },
        Case {
            name: "scrape may flip done to working",
            steps: vec![
                (Hook, Done, 0, Some(Done)),
                (Scrape, Working, 1_000, None),
            ],
            final_state: Working,
        },
        Case {
            name: "scrape accepted after hook window",
            steps: vec![
                (Hook, Working, 0, None),
                (Scrape, Done, 600_000, Some(Done)),
            ],
            final_state: Done,
        },
        Case {
            name: "scrape only pane",
            steps: vec![
                (Scrape, Working, 0, None),
                (Scrape, NeedsInput, 1_000, Some(NeedsInput)),
            ],
            final_state: NeedsInput,
        },
        Case {
            name: "exited does not alert",
            steps: vec![(Hook, Exited, 0, None)],
            final_state: Exited,
        },
    ];
    for c in cases {
        let mut t = Tracker::new(TrackerConfig::default());
        for (i, (src, st, ts, want)) in c.steps.iter().enumerate() {
            let got = t.apply(HOST, &ev(1, *src, *st, *ts)).map(|a| a.state);
            assert_eq!(got, *want, "case '{}' step {i}", c.name);
        }
        let rec = t.get(&key(1)).expect("record");
        assert_eq!(rec.state, c.final_state, "case '{}' final", c.name);
    }
}

#[test]
fn alert_carries_key_agent_and_message() {
    let mut t = Tracker::new(TrackerConfig::default());
    let mut e = ev(7, Hook, NeedsInput, 5);
    e.message = Some("allow bash?".into());
    let a = t.apply(HOST, &e).expect("alert");
    assert_eq!(a.key, key(7));
    assert_eq!(a.agent, "claude");
    assert_eq!(a.message.as_deref(), Some("allow bash?"));
}

#[test]
fn pending_orders_needs_input_first_then_by_time() {
    let mut t = Tracker::new(TrackerConfig::default());
    t.apply(HOST, &ev(1, Hook, Done, 100));
    t.apply(HOST, &ev(2, Hook, NeedsInput, 300));
    t.apply(HOST, &ev(3, Hook, NeedsInput, 200));
    t.apply(HOST, &ev(4, Hook, Working, 50));
    let order: Vec<u32> = t.pending().iter().map(|r| r.key.pane_id).collect();
    assert_eq!(order, vec![3, 2, 1]);
}

#[test]
fn acknowledge_removes_from_pending() {
    let mut t = Tracker::new(TrackerConfig::default());
    t.apply(HOST, &ev(1, Hook, Done, 0));
    assert!(t.acknowledge(&key(1)));
    assert!(t.pending().is_empty());
    assert!(!t.acknowledge(&key(99)));
}

#[test]
fn deduped_state_is_still_pending() {
    let mut t = Tracker::new(TrackerConfig::default());
    t.apply(HOST, &ev(1, Hook, Done, 0));
    t.apply(HOST, &ev(1, Hook, Working, 1_000));
    assert!(t.apply(HOST, &ev(1, Hook, Done, 2_000)).is_none());
    assert_eq!(t.pending().len(), 1);
}

#[test]
fn pane_gone_marks_exited_and_clears_pending() {
    let mut t = Tracker::new(TrackerConfig::default());
    t.apply(HOST, &ev(1, Hook, NeedsInput, 0));
    t.pane_gone(&key(1), 10);
    assert_eq!(t.get(&key(1)).unwrap().state, Exited);
    assert!(t.pending().is_empty());
}

#[test]
fn hosts_are_isolated() {
    let mut t = Tracker::new(TrackerConfig::default());
    t.apply("host-a", &ev(1, Hook, Done, 0));
    t.apply("host-b", &ev(1, Hook, Working, 0));
    assert_eq!(t.get(&key(1)).unwrap().state, Done);
}
```

- [ ] **Step 4: 运行测试，确认失败**

Run: `cargo test -p mai-core`
Expected: 编译失败，提示找不到 `mai_core::tracker`。

- [ ] **Step 5: 写 `crates/mai-core/src/lib.rs`**

```rust
//! UI-independent core of the multi-ai app.

pub mod tracker;
```

- [ ] **Step 6: 写 `crates/mai-core/src/tracker.rs`**

```rust
//! Per-agent state tracking: merges hook and scrape events and decides
//! when the user must be alerted.

use std::collections::HashMap;

use mai_protocol::{AgentEvent, AgentState, EventSource};

/// Identifies one agent: a pane in a zellij session on a host.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AgentKey {
    pub host_id: String,
    pub session: String,
    pub pane_id: u32,
}

impl AgentKey {
    pub fn from_event(host_id: &str, ev: &AgentEvent) -> Self {
        Self {
            host_id: host_id.to_owned(),
            session: ev.pane.session.clone(),
            pane_id: ev.pane.pane_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackerConfig {
    /// While a hook event is newer than this, scrape events may only
    /// move an idle agent back to Working.
    pub hook_authority_ms: u64,
    /// Re-entering the same alerting state within this window is silent.
    pub alert_dedupe_ms: u64,
}

impl Default for TrackerConfig {
    fn default() -> Self {
        Self { hook_authority_ms: 600_000, alert_dedupe_ms: 30_000 }
    }
}

/// Emitted when the user should be notified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub key: AgentKey,
    pub agent: String,
    pub state: AgentState,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRecord {
    pub key: AgentKey,
    pub agent: String,
    pub state: AgentState,
    pub message: Option<String>,
    /// False while an alerting state has not been seen by the user.
    pub acknowledged: bool,
    /// Timestamp at which `state` was entered.
    pub since_ms: u64,
    last_hook_ms: Option<u64>,
    last_alert: Option<(AgentState, u64)>,
}

#[derive(Debug)]
pub struct Tracker {
    cfg: TrackerConfig,
    agents: HashMap<AgentKey, AgentRecord>,
}

fn is_alerting(s: AgentState) -> bool {
    matches!(s, AgentState::NeedsInput | AgentState::Done)
}

fn scrape_may_override(from: AgentState, to: AgentState) -> bool {
    is_alerting(from) && to == AgentState::Working
}

impl Tracker {
    pub fn new(cfg: TrackerConfig) -> Self {
        Self { cfg, agents: HashMap::new() }
    }

    /// Apply one event; returns an alert when the user should be notified.
    pub fn apply(&mut self, host_id: &str, ev: &AgentEvent) -> Option<Alert> {
        let key = AgentKey::from_event(host_id, ev);
        let cfg = self.cfg;
        let rec = self.agents.entry(key.clone()).or_insert_with(|| {
            AgentRecord {
                key: key.clone(),
                agent: ev.agent.clone(),
                state: AgentState::Unknown,
                message: None,
                acknowledged: true,
                since_ms: ev.ts_ms,
                last_hook_ms: None,
                last_alert: None,
            }
        });
        match ev.source {
            EventSource::Hook => rec.last_hook_ms = Some(ev.ts_ms),
            EventSource::Scrape => {
                let authoritative = rec.last_hook_ms.is_some_and(|h| {
                    ev.ts_ms.saturating_sub(h) < cfg.hook_authority_ms
                });
                if authoritative && !scrape_may_override(rec.state, ev.state) {
                    return None;
                }
            }
        }
        if rec.state == ev.state {
            return None;
        }
        rec.agent = ev.agent.clone();
        rec.state = ev.state;
        rec.message = ev.message.clone();
        rec.since_ms = ev.ts_ms;
        if !is_alerting(ev.state) {
            rec.acknowledged = true;
            return None;
        }
        rec.acknowledged = false;
        if let Some((s, t)) = rec.last_alert
            && s == ev.state
            && ev.ts_ms.saturating_sub(t) < cfg.alert_dedupe_ms
        {
            return None;
        }
        rec.last_alert = Some((ev.state, ev.ts_ms));
        Some(Alert {
            key,
            agent: rec.agent.clone(),
            state: ev.state,
            message: rec.message.clone(),
        })
    }

    /// The pane disappeared or exited.
    pub fn pane_gone(&mut self, key: &AgentKey, now_ms: u64) {
        if let Some(rec) = self.agents.get_mut(key) {
            rec.state = AgentState::Exited;
            rec.since_ms = now_ms;
            rec.acknowledged = true;
        }
    }

    /// Mark the current alert as seen. Returns false for unknown keys.
    pub fn acknowledge(&mut self, key: &AgentKey) -> bool {
        match self.agents.get_mut(key) {
            Some(rec) => {
                rec.acknowledged = true;
                true
            }
            None => false,
        }
    }

    pub fn get(&self, key: &AgentKey) -> Option<&AgentRecord> {
        self.agents.get(key)
    }

    /// Unacknowledged alerting agents: NeedsInput first, then Done,
    /// each group oldest first.
    pub fn pending(&self) -> Vec<&AgentRecord> {
        let mut v: Vec<&AgentRecord> = self
            .agents
            .values()
            .filter(|r| is_alerting(r.state) && !r.acknowledged)
            .collect();
        v.sort_by_key(|r| {
            let rank = u8::from(r.state != AgentState::NeedsInput);
            (rank, r.since_ms, r.key.pane_id)
        });
        v
    }
}
```

- [ ] **Step 7: 运行测试，确认通过**

Run: `cargo test -p mai-core`
Expected: 7 个测试全部 PASS。

- [ ] **Step 8: clippy 零警告**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: 无警告。

- [ ] **Step 9: README 的 Crates 表加一行**

```markdown
| `mai-core` | UI-independent core: agent state tracking and alerts |
```

- [ ] **Step 10: 提交**

```bash
git add Cargo.toml Cargo.lock README.md crates/mai-core
git commit -m "feat(core): add agent state tracker with alert dedupe"
```

---

### Task 3: mai-probe 抓屏识别与状态推断

**Files:**

- Modify: `Cargo.toml`（members 加入 `crates/mai-probe`）
- Modify: `README.md`（Crates 表加一行）
- Create: `crates/mai-probe/Cargo.toml`
- Create: `crates/mai-probe/src/lib.rs`
- Create: `crates/mai-probe/src/scrape.rs`
- Test: `crates/mai-probe/tests/scrape.rs`

**Interfaces:**

- Consumes: `mai_protocol::{AgentRule, AgentState, PaneRef, ScrapeRules}`
- Produces（02-probe 使用）：
  - `CompiledRules::compile(&ScrapeRules) -> Result<CompiledRules, regex::Error>`
  - `CompiledRules::identify(&self, title: &str, command: Option<&str>,`
    `screen: &str) -> Option<&str>`：返回第一条命中规则的 agent 名称
  - `ScrapeTracker::default()`
  - `ScrapeTracker::observe(&mut self, rules: &CompiledRules, agent: &str,`
    `pane: &PaneRef, screen: &str, now_ms: u64) -> Option<AgentState>`
  - `ScrapeTracker::forget(&mut self, pane: &PaneRef)`

**推断规则（设计第 6.2 节）：**

1. 对某个 pane 的第一次观察只记录基线，不输出任何状态。
2. 屏幕哈希与上次不同：记为 `Working`，并重置稳定起点。
3. 屏幕哈希不变且 `now - 稳定起点 >= stable_ms`：命中
   `needs_input_patterns` 则为 `NeedsInput`，否则命中 `done_patterns`
   则为 `Done`，都不命中则不输出。
4. 同一个 pane 连续推断出相同状态时，只输出一次。

- [ ] **Step 1: 修改 workspace 根 `Cargo.toml`**

```toml
members = ["crates/mai-protocol", "crates/mai-core", "crates/mai-probe"]
```

- [ ] **Step 2: 创建 `crates/mai-probe/Cargo.toml`**

```toml
[package]
name = "mai-probe"
version.workspace = true
edition.workspace = true

[dependencies]
mai-protocol = { workspace = true }
regex = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 3: 写失败测试 `crates/mai-probe/tests/scrape.rs`**

```rust
use mai_probe::scrape::{CompiledRules, ScrapeTracker};
use mai_protocol::{AgentRule, AgentState, PaneRef, ScrapeRules};

fn rules() -> CompiledRules {
    CompiledRules::compile(&ScrapeRules {
        agents: vec![AgentRule {
            name: "fake".into(),
            title_patterns: vec!["^fakeagent".into()],
            screen_patterns: vec![r"FAKE AGENT v\d".into()],
            needs_input_patterns: vec![r"(?m)^Allow\? \[y/n\]".into()],
            done_patterns: vec![r"(?m)^> $".into()],
            stable_ms: 5_000,
        }],
    })
    .unwrap()
}

fn pane() -> PaneRef {
    PaneRef { session: "work".into(), pane_id: 1 }
}

const ASK: &str = "FAKE AGENT v1\nrun rm?\nAllow? [y/n]\n";
const IDLE: &str = "FAKE AGENT v1\nall done\n> \n";

#[test]
fn identify_by_title_command_or_screen() {
    let r = rules();
    assert_eq!(r.identify("fakeagent - x", None, ""), Some("fake"));
    assert_eq!(r.identify("bash", Some("fakeagent --x"), ""), Some("fake"));
    assert_eq!(r.identify("bash", None, "FAKE AGENT v2 ready"), Some("fake"));
    assert_eq!(r.identify("bash", None, "$ ls"), None);
}

#[test]
fn compile_rejects_bad_regex() {
    let bad = ScrapeRules {
        agents: vec![AgentRule {
            name: "x".into(),
            title_patterns: vec!["(".into()],
            screen_patterns: vec![],
            needs_input_patterns: vec![],
            done_patterns: vec![],
            stable_ms: 1,
        }],
    };
    assert!(CompiledRules::compile(&bad).is_err());
}

#[test]
fn first_observation_is_baseline_only() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    assert_eq!(t.observe(&r, "fake", &pane(), IDLE, 0), None);
}

#[test]
fn change_emits_working_once() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "fake", &pane(), "a", 0);
    let got = t.observe(&r, "fake", &pane(), "b", 1_000);
    assert_eq!(got, Some(AgentState::Working));
    assert_eq!(t.observe(&r, "fake", &pane(), "c", 2_000), None);
}

#[test]
fn stable_prompt_becomes_needs_input_after_stable_ms() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "fake", &pane(), "a", 0);
    let got = t.observe(&r, "fake", &pane(), ASK, 1_000);
    assert_eq!(got, Some(AgentState::Working));
    assert_eq!(t.observe(&r, "fake", &pane(), ASK, 3_000), None);
    let got = t.observe(&r, "fake", &pane(), ASK, 6_000);
    assert_eq!(got, Some(AgentState::NeedsInput));
    assert_eq!(t.observe(&r, "fake", &pane(), ASK, 7_000), None);
}

#[test]
fn stable_idle_prompt_becomes_done() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "fake", &pane(), IDLE, 0);
    let got = t.observe(&r, "fake", &pane(), IDLE, 5_000);
    assert_eq!(got, Some(AgentState::Done));
}

#[test]
fn stable_without_match_emits_nothing() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "fake", &pane(), "thinking", 0);
    assert_eq!(t.observe(&r, "fake", &pane(), "thinking", 9_000), None);
}

#[test]
fn unknown_agent_returns_none() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "nope", &pane(), "a", 0);
    assert_eq!(t.observe(&r, "nope", &pane(), "b", 1_000), None);
}

#[test]
fn forget_resets_baseline() {
    let (r, mut t) = (rules(), ScrapeTracker::default());
    t.observe(&r, "fake", &pane(), "a", 0);
    t.forget(&pane());
    assert_eq!(t.observe(&r, "fake", &pane(), "b", 1_000), None);
}
```

- [ ] **Step 4: 运行测试，确认失败**

Run: `cargo test -p mai-probe`
Expected: 编译失败，提示找不到 `mai_probe::scrape`。

- [ ] **Step 5: 写 `crates/mai-probe/src/lib.rs`**

```rust
//! mai-probe library: logic used by the probe binary.

pub mod scrape;
```

- [ ] **Step 6: 写 `crates/mai-probe/src/scrape.rs`**

```rust
//! Screen-scrape fallback: identifies agent panes and infers agent state
//! from zellij screen dumps.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use mai_protocol::{AgentRule, AgentState, PaneRef, ScrapeRules};
use regex::Regex;

struct CompiledRule {
    name: String,
    stable_ms: u64,
    title: Vec<Regex>,
    screen: Vec<Regex>,
    needs_input: Vec<Regex>,
    done: Vec<Regex>,
}

fn compile_all(patterns: &[String]) -> Result<Vec<Regex>, regex::Error> {
    patterns.iter().map(|p| Regex::new(p)).collect()
}

fn any_match(res: &[Regex], text: &str) -> bool {
    res.iter().any(|r| r.is_match(text))
}

impl CompiledRule {
    fn compile(r: &AgentRule) -> Result<Self, regex::Error> {
        Ok(Self {
            name: r.name.clone(),
            stable_ms: r.stable_ms,
            title: compile_all(&r.title_patterns)?,
            screen: compile_all(&r.screen_patterns)?,
            needs_input: compile_all(&r.needs_input_patterns)?,
            done: compile_all(&r.done_patterns)?,
        })
    }
}

/// Rules with all regexes compiled once.
pub struct CompiledRules {
    rules: Vec<CompiledRule>,
}

impl CompiledRules {
    pub fn compile(src: &ScrapeRules) -> Result<Self, regex::Error> {
        let rules = src
            .agents
            .iter()
            .map(CompiledRule::compile)
            .collect::<Result<_, _>>()?;
        Ok(Self { rules })
    }

    /// Name of the first agent whose title patterns match the pane title
    /// or command, or whose screen patterns match the screen.
    pub fn identify(
        &self,
        title: &str,
        command: Option<&str>,
        screen: &str,
    ) -> Option<&str> {
        self.rules
            .iter()
            .find(|r| {
                any_match(&r.title, title)
                    || command.is_some_and(|c| any_match(&r.title, c))
                    || any_match(&r.screen, screen)
            })
            .map(|r| r.name.as_str())
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
        let hash = hash_screen(screen);
        let Some(p) = self.panes.get_mut(pane) else {
            self.panes.insert(
                pane.clone(),
                PaneScrape { hash, stable_since_ms: now_ms, emitted: None },
            );
            return None;
        };
        let next = if p.hash != hash {
            p.hash = hash;
            p.stable_since_ms = now_ms;
            AgentState::Working
        } else if now_ms.saturating_sub(p.stable_since_ms) < rule.stable_ms {
            return None;
        } else if any_match(&rule.needs_input, screen) {
            AgentState::NeedsInput
        } else if any_match(&rule.done, screen) {
            AgentState::Done
        } else {
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
}
```

- [ ] **Step 7: 运行测试，确认通过**

Run: `cargo test -p mai-probe`
Expected: 9 个测试全部 PASS。

- [ ] **Step 8: 全 workspace 验证**

Run:

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 构建无警告；22 个测试全部 PASS；clippy 无警告。

- [ ] **Step 9: README 的 Crates 表加一行**

```markdown
| `mai-probe` | remote probe; currently the screen-scrape classifier |
```

- [ ] **Step 10: 提交**

```bash
git add Cargo.toml Cargo.lock README.md crates/mai-probe
git commit -m "feat(probe): add screen-scrape agent classifier"
```

## 已知限制（留给后续计划）

- 空闲时屏幕上仍在变化的内容（如时钟、动画）会被误判为 `Working`。
  是否需要“忽略行”规则，由 spike 采集的屏幕样本决定（02-probe）。
- hook 载荷到 `AgentEvent` 的映射、默认 `rules.toml` 的内容，放到 02-probe。
