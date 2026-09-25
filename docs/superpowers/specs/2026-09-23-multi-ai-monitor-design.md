# multi-ai 设计文档

- 日期：2026-09-23
- 状态：设计已确认，待用户审阅本文件
- 需求分析：[2026-09-23-multi-ai-monitor-analysis.md](2026-09-23-multi-ai-monitor-analysis.md)

## 1. 概述

multi-ai 是一个基于 Tauri 2 的桌面应用（macOS / Windows）。它通过 SSH 连接多台主机（含本机），
在应用内嵌终端中 attach 各主机的 zellij session，并在每台主机上运行探针 `mai-probe`，
监控 zellij pane 中 AI agent 的状态。agent 需要用户处理时，应用发出提醒；
用户点击后直接跳到对应 pane 继续交互。

## 2. 总体架构

```text
+-------------------- 本地应用 (Tauri) --------------------+
|  UI (TypeScript + xterm.js)                              |
|   主机/agent 列表 | 终端区 | 快捷文本栏 | 监控面板        |
|        ^ Tauri events / commands                         |
|  mai-core (Rust, 与 UI 无关)                             |
|   HostManager -> 每主机: TermConn + ProbeConn            |
|   AgentStateMachine / Notifier / Keychain / Config       |
+----------------------------------------------------------+
        | SSH 连接 1 (TermConn)     | SSH 连接 2 (ProbeConn)
        v                           v
+---------------- 远程主机 (linux/mac/win) ----------------+
|  zellij attach <session> (PTY)   mai-probe serve         |
|                                   - 轮询 list-sessions / |
|                                     list-panes           |
|                                   - 读 hook 事件 spool   |
|                                   - 兜底 dump-screen     |
|                                   - 采集 CPU/IO/网络     |
|  agent hooks -> mai-probe hook/emit -> spool 目录        |
+----------------------------------------------------------+
```

### 2.1 Rust workspace

| crate | 类型 | 职责 | 依赖 |
| --- | --- | --- | --- |
| `mai-protocol` | lib | 探针与应用之间的消息类型（serde），协议版本常量 | serde |
| `mai-probe` | bin | 远程探针：serve、hook、emit 等子命令 | mai-protocol, sysinfo |
| `mai-core` | lib | 连接、部署、状态机、通知抽象、钥匙串、配置 | 见下方 |
| `mai-app` | bin | Tauri 外壳：暴露 mai-core，承载前端 | mai-core, tauri |

mai-core dependencies: mai-protocol, russh (ring), russh-sftp, ssh2-config,
keyring, tokio, sha2, serde

前端代码位于 `mai-app/ui/`（TypeScript + xterm.js）。

`mai-probe` 交叉编译目标：

- `x86_64-unknown-linux-musl`
- `aarch64-unknown-linux-musl`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`

应用安装包内置全部探针二进制，部署无需联网。

### 2.2 Transport 抽象

`mai-core` 定义 `Transport` trait，统一远程与本机：

| 能力 | 远程实现（russh） | 本机实现 |
| --- | --- | --- |
| 执行命令（exec，取 stdin/stdout） | SSH exec channel | 子进程 |
| 打开 PTY 运行命令 | SSH PTY channel | portable-pty（Windows 为 ConPTY） |
| 上传文件 | SFTP，失败回退 exec 写文件 | 本地文件复制 |

每台远程主机建立两条独立 SSH 连接：

- **TermConn**：承载交互终端。一个 zellij session 对应一个 PTY channel，多个 session 复用这条连接。
- **ProbeConn**：承载探针的 exec channel。

两条连接相互隔离：探针流量不影响交互延迟，任一条断开各自重连。

本机不经 SSH：探针复制到 `<home>/.mai/bin/` 后作为子进程运行
（Windows 上使用 `CREATE_NO_WINDOW`，不弹控制台窗口），经子进程的
stdin/stdout 通信；本机不需要 sshd。

## 3. 连接与认证

### 3.1 主机配置

- 在 UI 中手动添加，或从 `~/.ssh/config` 导入 Host 别名。
- 自行解析 `~/.ssh/config`（ssh2-config；跳板主机自身的 `ProxyJump` 不再展开），
  支持：HostName、User、Port、IdentityFile、ProxyJump。
- 每台主机可选配置：zellij 可执行文件路径、默认 session、探针轮询间隔。

### 3.2 认证顺序

先以 `none` 查询服务器允许的方法，未提供的方法跳过。

1. 配置或 ssh config 中指定的私钥（口令从钥匙串读取，缺失则弹窗输入并询问是否保存）。
2. ssh-agent（macOS：`SSH_AUTH_SOCK`；Windows：OpenSSH Agent 命名管道，Pageant 可选）。
3. 钥匙串中保存的密码。
4. 弹窗输入密码；keyboard-interactive / 2FA 弹窗逐项交互。

密码与口令失效时从钥匙串删除。密码与口令只存系统钥匙串（`keyring` crate），
服务名 `multi-ai`，键为 `<host>/password` 与 `passphrase:<私钥路径>`。

### 3.3 主机密钥校验

- 读取 `~/.ssh/known_hosts`（以及应用自己的 known_hosts 文件）。
- 未知主机：弹窗显示指纹，用户确认后写入应用的 known_hosts。
- 指纹不一致：拒绝连接并醒目提示，不提供“忽略”按钮。

### 3.4 连接状态与重连

主机状态：`Connecting`、`Online`、`Degraded`（仅一条连接可用）、`Offline`、`AuthRequired`。
每条连接断开后独立指数退避重连（1s 起，上限 60s）；`AuthRequired` 不自动重试，等待用户操作。

- 每台主机由一个任务负责探针连接：连接、部署、启动 `serve`、转发消息、重连。
  同一主机的连接与部署因此不会并发；另有按主机的锁，终端连接（03c）连接时
  同样持有，避免同一主机同时弹出两次主机密钥确认。
- 每条连接的状态：`Connecting`、`Up`、`Retrying`（附下次重试的等待时间与原因）、
  `NeedsUser`（附原因）。主机状态由两条连接的状态推出；终端连接出现之前只看探针连接。
- 需要用户处理、不自动重试的原因：认证失败、拒绝主机密钥、主机密钥变化、
  部署失败（如缺少对应平台的探针、上传失败）、配置错误（如 ssh config 无法解析）、
  探针协议版本不符。用户点“重试”后立即重连。
- 网络类错误（TCP、密钥交换、认证过程中连接断开、通道错误）按退避自动重试。
  探针连续在线超过 30 秒后断开，退避重新从 1 秒开始；刚启动就断开的探针继续加倍等待。
- hook 安装失败不影响连接：探针照常运行（仅靠抓屏识别），失败原因交给 UI 显示。

## 4. 探针 mai-probe

### 4.1 子命令

| 子命令 | 调用方 | 作用 |
| --- | --- | --- |
| `serve [选项]` | 应用启动 | 随 channel 存活，经 stdio 通信 |
| `stop [--client <id>]` | 应用部署前 | 结束该 client 正在运行的 `serve`（见 4.6） |
| `hook <agent>` | Claude Code / Codex hooks | 事件名取自载荷，写 spool 即返回 |
| `emit --agent <name> --state <s> [--msg <m>]` | cmagent 及其他 agent | 通用上报接口 |
| `install-hooks` / `uninstall-hooks` | 应用在部署后调用 | 合并式修改 agent 配置 |
| `--version` | 手工检查 | 输出版本（部署靠 SHA-256 比较，不需要构建哈希） |

`serve` 的选项：`--zellij <path>`（zellij 位置，见 4.5）、`--rules <file>`
（抓屏规则，默认内置规则）、`--client <id>`（应用安装标识，见 4.3、4.6）。

hook 条目以探针路径 `.mai/bin/mai-probe` 识别为本应用所有。因此 `install-hooks`
要求探针从 `.mai/bin` 下的绝对路径运行；否则每个 agent 输出 `outcome` 为 `error`
的结果行，不写入任何文件，退出码为 1。

### 4.2 部署流程

1. 探测：`uname -sm` 成功为 Linux/macOS；否则以 `echo %OS%` 区分 Windows 的
   cmd（输出 `Windows_NT`）与 PowerShell（原样输出），再取 CPU 与 home。
2. 以远程命令计算 `<home>/.mai/bin/mai-probe` 的 SHA-256
   （`sha256sum`、`shasum -a 256`、`certutil`、`Get-FileHash`），
   与应用内置二进制比较；一致则跳过上传。
3. 不一致或不存在：若旧文件存在，先执行旧探针的 `stop --client <id>`（尽力而为，
   旧版本没有该子命令时忽略）；SFTP 上传到 `<exe>.upload`；非 Windows 上 chmod 0755；
   若目标文件已存在则先删除，再将 `<exe>.upload` 重命名为目标名。
   SFTP 路径相对登录目录（`.mai/bin/...`）；内置二进制来自 CI 产物 `probes/<target>/`。
   上传后的校验、以及 SFTP 不可用时回退 exec 写入均尚未实现；
   “先删后 rename”之间还有一段窗口不是原子的，见
   `docs/superpowers/plans/2026-09-24-02-followups.md`（B18）。
4. 执行 `mai-probe install-hooks`（幂等）。
5. 执行 `mai-probe serve --client <id>`。`<id>` 是本应用安装的标识，只保留
   `[A-Za-z0-9_-]`；为空时不传该参数（PowerShell 5.1 会丢弃空参数）。
   远程命令经 SSH env 请求设置 `LANG`、`LC_CTYPE` 为 `en_US.UTF-8`。

本机按同样的步骤处理，只是改为本地文件复制与子进程（见 2.2）。

### 4.3 spool 目录

- 位置：`<数据目录>/spool/`，每个事件一个 JSON 行，按天分文件，追加写入。
  文件名为自 epoch 起的 UTC 天数 `<day>.jsonl`；游标为 `(day << 40) | 行尾偏移`，
  跨文件严格递增。
- 数据目录：探针位于 `<dir>/bin/` 且 `<dir>` 的名字以 `.mai` 开头（部署目录，
  如 `~/.mai`）时就是 `<dir>`；否则为 `MAI_HOME`，再否则为 `<home>/.mai`。
  hook 与 `serve` 运行的是同一个已部署的二进制，因此即使两者的环境变量不同，
  也总是读写同一个目录。
- hook 子命令只追加写，不做网络与重计算，保证不拖慢 agent。
- `serve` 启动时从上次确认的偏移量重放，运行中持续 tail。
- 应用把 hook 事件交给监控任务后立即回 `Ack`；`serve` 把偏移量写入
  `ack-<client>`（没有 client 时为 `ack`），不同应用各自一份，互不影响。
  写入经以进程号命名的临时文件再重命名。
- UTC 零点后 10 秒内不读新一天的文件：hook 在零点前取的时间戳可能在零点后
  才写入前一天的文件，读取方一旦进入新的一天就不再回看旧文件。
- `serve` 每小时检查一次，删除超过 7 天的 spool 文件（启动后的第一次检查立即执行）。

### 4.4 hook 安装（合并式修改）

- `~/.claude/settings.json`：在对应事件下追加带 `mai` 标识的 hook 条目；保留用户已有条目；
  修改前备份为 `settings.json.mai-bak-<时间戳>`。
- `~/.codex/hooks.json`：与 Claude 相同的合并方式追加带 `mai` 标识的条目。
  不使用 `notify`：它会被 Codex 内部轮次（如生成标题）触发，产生误报。
  Codex 的非托管 hooks 需用户信任后才运行（在 Codex 中执行 `/hooks`）；
  探针无法检测 Codex 是否已信任这些 hooks；`install-hooks` 对 Codex 的结果行带有
  `note` 字段，提示用户在 Codex 中执行 `/hooks`，由应用展示该提示。
- `uninstall-hooks` 仅移除带 `mai` 标识的条目。
- 解析失败（配置文件格式异常）时不写入，返回错误，由应用提示用户。

### 4.5 zellij 环境检查

- `serve` 启动后按顺序查找 zellij：`--zellij` 参数、PATH、常见安装位置
  （`/opt/homebrew/bin`、`/usr/local/bin`、`~/.cargo/bin`、`~/.local/bin`）、
  登录 shell（`$SHELL -lc 'command -v zellij'`）。
  非交互 SSH exec 的 PATH 常不含这些位置（spike 在 macOS 上证实）。
- 显式给出的 `--zellij` 路径不存在时，直接按“找不到”处理，不再回退到 PATH、
  常见位置或登录 shell。
- 找不到：`Hello` 中报告 `zellij: null`，应用提示用户“将 zellij 加入 PATH 或在主机设置中填写路径”，
  该主机的监控停在此步，不做猜测。
- 每条 zellij 命令最多运行 5 秒，超时即结束该进程并报错，不会卡住 `serve` 循环。
  Windows 上以 `CREATE_NO_WINDOW` 启动。
- 周期性操作（列 session、列 pane、抓屏、清理 spool）的错误按操作对象只在
  开始失败时报告一次，恢复成功后再次失败才会再报告。

### 4.6 单实例

- 半开的 SSH 连接可能留下旧的 `serve`；在 Windows 上它还锁住探针文件，导致无法重新部署。
- 每个 `serve` 把自己的进程号写入 `<数据目录>/serve-<client>.pid`。新的 `serve`
  启动时（以及 `mai-probe stop`）结束该文件记录的进程：只在该进程仍存活且名字以
  `mai-probe` 开头时才结束，最多等 3 秒。不同 client 的 `serve` 可以同时运行。
- `serve` 正常退出时，若 pid 文件仍是自己的进程号则删除它；被强制结束时残留的
  pid 文件无害。
- 另一个 client 的 `serve` 仍在运行时，Windows 上替换探针会失败，部署报错并等待用户处理。

## 5. 通信协议 mai-protocol

- 传输：探针 stdout 输出、应用写入探针 stdin，均为每行一个 JSON 对象（JSON Lines）。
- 每条消息含 `type` 字段；协议版本 `PROTOCOL_VERSION: u32`。
- 握手：探针首先发送 `Hello`；应用比对协议版本，不兼容则等待用户处理
  （部署已按 SHA-256 保证探针与应用内置的一致，版本仍不符说明安装包有误）。
- `Hello` 之前最多容忍 50 行非协议输出（如 shell 启动文件打印的内容）；
  30 秒内没有 `Hello`，或之后 20 秒没有任何消息（心跳每 5 秒一次），应用断开并重连。
- 无法解析的行转为 `code` 为 `bad_probe_line` 的 `Error`，不中断连接。

### 5.1 探针到应用

| 消息 | 内容 |
| --- | --- |
| `Hello` | protocol_version、probe_version、os、arch、zellij_path、zellij_version |
| `Sessions` | 当前全部 zellij session 列表（名称、是否存活） |
| `Panes` | 某 session 的 pane 列表（id、tab_id、tab_name、title、command、exited） |
| `AgentEvent` | session、pane_id、agent、source、state、message、时间、spool 偏移 |
| `Metrics` | cpu、mem、disk_io、net_io、load、timestamp |
| `Heartbeat` | timestamp |
| `Error` | code、message |

### 5.2 应用到探针

| 消息 | 作用 |
| --- | --- |
| `Ack` | 确认已收到的 spool 偏移 |
| `SendText` | session、pane_id、text；探针执行 `zellij action paste -p`（第二期使用） |
| `Focus` | session、pane_id、tab_id；执行 go-to-tab-by-id 与 focus-pane-id（见 8.2） |
| `SetInterval` | 调整 pane 轮询、抓屏、指标采集间隔 |
| `SetRules` | 下发抓屏规则（见 6.2），探针收到后立即替换 |

## 6. Agent 识别与状态机

### 6.1 标识与状态

- Agent 唯一标识：`(host_id, session_name, pane_id)`。
- 状态：`Unknown`、`Working`、`NeedsInput`、`Done`、`Exited`。

### 6.2 信号来源

1. **hook 上报（权威）**

   | agent | 事件 | 映射状态 |
   | --- | --- | --- |
   | Claude Code | UserPromptSubmit、PreToolUse、PostToolUse | Working |
   | Claude Code | Notification（`permission_prompt`） | NeedsInput |
   | Claude Code | Stop；Notification（`idle_prompt`） | Done |
   | Claude Code | SessionEnd | Exited |
   | Codex | UserPromptSubmit、PreToolUse、PostToolUse | Working |
   | Codex | PermissionRequest | NeedsInput |
   | Codex | Stop、Interrupt | Done |
   | Codex | SessionEnd | Exited |
   | 其他 | `mai-probe emit --state <s>` | 按参数 |

   事件与载荷样本见 `docs/superpowers/spike-data/`，
   详见 `2026-09-23-spike-findings.md`（U3、U4）。

2. **抓屏兜底**：对无 hook 覆盖的 agent pane，按间隔执行 `zellij action dump-screen -p <pane>`：
   - 先删除匹配忽略规则的行（状态栏倒计时、空闲动画等），再计算哈希；
     spike 证实 Claude 与 Codex 空闲时屏幕仍会变化。
   - 屏幕内容哈希持续变化，判为 Working。
   - 稳定超过 N 秒（默认 5）且匹配 NeedsInput 规则，判为 NeedsInput；匹配 Done 规则，判为 Done。
   - 规则按 agent 写在规则文件（TOML，正则），应用下发给探针，可热更新。

3. **agent pane 识别**（任一满足）：
   - 该 pane 曾产生 hook 事件；
   - pane 标题或命令匹配规则；命令优先取 `list-panes -a` 的 `pane_command`（当前前台命令）；
   - 屏幕内容匹配 agent 特征规则。

   普通 shell pane 不在 UI 列表中显示。

4. **绑定的保持与解除**：
   - 仅由抓屏识别的 agent，只要它自己的任一规则（标题或命令、屏幕特征、
     needs-input、done）仍匹配就保持绑定；连续 2 次抓屏都不匹配才报告 `Exited`。
     这样 Claude 在弹窗（计划确认、AskUserQuestion 等）下不会被误判退出。
   - hook 报告 `Exited` 后，该 pane 至少要出现一次“什么 agent 都识别不到”的抓屏，
     才允许抓屏重新绑定；否则残留的标题或最后一屏会让它被重新识别，
     随后产生一次虚假的 `Done` 提醒。

### 6.3 合并规则

- 某 pane 最近 10 分钟内有 hook 事件时，以 hook 为准；抓屏只可产生 hook 无法表达的转换
  （例如 Codex 在 Done 后用户再次输入，由抓屏判为 Working）。
- pane 在 `Panes` 中消失或 `exited=true` 时，判为 Exited。

### 6.4 提醒

- 触发：状态转入 `NeedsInput` 或 `Done`。
- 形式：应用内（图标颜色、角标、列表置顶）、系统通知（tauri-plugin-notification）、声音。
- `NeedsInput` 优先级高于 `Done`，使用不同颜色与声音。
- 去重：同一 agent 同一状态在 30 秒内重复触发只提醒一次。
- 确认：用户打开该 pane（点击列表或通知）后，提醒标记为已确认。
- 各提醒形式可在设置中分别开关。

## 7. 交互终端

- 每个已打开的 `(host, session)` 对应 UI 中一个终端标签，内容为 xterm.js。
- 远程：TermConn 上开 PTY channel，`TERM=xterm-256color`，执行 `<zellij> attach <session>`。
- 远程命令必须使用 UTF-8 locale：优先经 SSH env 请求发送 `LANG`/`LC_CTYPE`，
  服务器拒绝时在命令前加 `LANG=... LC_CTYPE=...`。否则 macOS 上中文输入异常。
- xterm.js 必须加载 `@xterm/addon-clipboard`（zellij 复制走 OSC 52）与
  `@xterm/addon-unicode11`（宽字符宽度与 zellij 一致）。
- 本机：portable-pty 执行同样命令。
- 窗口尺寸变化同步到 PTY（window-change）。
- 字节流：PTY 输出经 Tauri event 推给前端，前端键盘输入经 Tauri command 写回 PTY。

## 8. 点击跳转

### 8.1 流程

1. 若 `(host, session)` 终端标签不存在，则创建并 attach。
2. 切换到该标签。
3. 发送 `Focus` 定位 tab 与 pane。
4. 标记该 agent 的提醒为已确认。

### 8.2 风险

spike（U1）证实：外部 `zellij action focus-pane-id`、`go-to-tab-*` 只作用于
最近有输入的客户端。用户只在应用内操作时，跳转正确；若用户同时在其他终端
attach 同一 session 并有输入，跳转会作用到那个终端。
04-app 需先实验能否让应用的客户端成为最近活跃客户端（如焦点事件、尺寸变化）；
不可靠时改为在应用的 PTY 中发送 zellij 按键序列完成跳转。

## 9. UI

```text
+----------------+-----------------------------------------+
| 主机 / Agent   |  [host-a:work] [host-b:dev] [local:w]   |
|                |-----------------------------------------|
| v host-a  (on) |                                         |
|   work         |                                         |
|    ! claude #3 |        xterm.js (zellij attach)         |
|    * codex  #7 |                                         |
|   dev          |                                         |
|    . claude #2 |                                         |
| v host-b (off) |                                         |
| v local   (on) |-----------------------------------------|
|                |  [继续] [yes] [/compact] [...] 快捷栏   |
+----------------+-----------------------------------------+
| host-a CPU 34% MEM 61% NET 1.2MB/s                       |
+----------------------------------------------------------+
图例: ! NeedsInput   * Done   . Working
```

- 左侧：主机、session、agent 三级树；需处理的 agent 置顶；提供“只看需处理”过滤。
- 顶部：已打开的终端标签。
- 底部：当前主机的简单指标。
- 快捷栏（第二期）：按钮最小 44px 高，支持横向滑动，面向触摸屏。

## 10. 配置与数据

- 配置目录（`directories` crate）：
  - macOS：`~/Library/Application Support/multi-ai/`
  - Windows：`%APPDATA%\multi-ai\`
- `config.toml`：主机列表、每主机 zellij 路径、通知与声音开关、快捷文本（第二期）。
- `rules.toml`：抓屏规则，随应用提供默认值，用户可覆盖。
- `known_hosts`：应用确认过的主机密钥。
- 秘密信息只在系统钥匙串中。

## 11. 错误处理原则

- 所有错误带上下文（主机、连接、步骤），在 UI 对应主机上显示，不静默吞掉。
- 探针部署、hook 安装失败时，该主机降级为仅终端可用，并显示原因与重试入口。
- 探针退出或 channel 断开：由 ProbeConn 重连后重新部署检查、重启 `serve`，spool 保证事件不丢。
- 配置文件解析失败：拒绝启动对应功能并指出文件与行号，不使用默认值覆盖用户文件。

## 12. 测试策略

| 对象 | 方式 |
| --- | --- |
| `mai-protocol` | 序列化/反序列化往返单元测试 |
| 状态机 | 表驱动测试：输入事件序列，断言状态与提醒输出 |
| 抓屏规则 | 以真实屏幕快照为用例的规则测试 |
| hook 安装 | 对各类已有配置文件（空、已有 hooks、格式错误）的合并测试 |
| `mai-probe serve` | 本机端到端测试（本机有 zellij） |
| SSH | 用本机/CI 的 OpenSSH 服务做集成测试 |

## 13. 分期

| 期 | 范围 |
| --- | --- |
| 第 0 期 spike | 验证 U1-U5（见需求分析第 6 节），产出结论，代码作废 |
| 第一期 MVP | 主机管理与连接、内嵌终端、探针部署、hook + 抓屏检测、状态机、三种提醒、底部简单指标 |
| 第二期 | 快捷文本栏与触摸优化、完整性能监控面板 |
| 第三期 | 推送到手机（ntfy、Bark 等） |

## 14. 约束

- 应用与库源码只含 ASCII 字符（代码、字符串、注释）；UI 中文文案放在独立的资源文件中。
- 编译零警告（Rust 与 TypeScript）。
- 代码变更后同步更新用户文档。
