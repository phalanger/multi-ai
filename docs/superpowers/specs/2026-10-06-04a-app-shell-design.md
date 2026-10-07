# 04a 应用外壳 设计

总设计：`2026-09-23-multi-ai-monitor-design.md`（下称“总设计”）。本文只描述
04a；04b（agent 树、提醒、跳转、状态持久化）与 04c（指标、设置页、清理与
其余 S 项）另行设计。

## 1. 目标与范围

### 1.1 用户意图（已确认）

- 04 拆成三段：04a 外壳与终端、04b agent 与提醒、04c 指标与设置。
- 04a 做完是一个可用的“多主机 zellij 终端”：在一个窗口里管理本机与 SSH 主机，
  看到每台主机的连接状态与 zellij session，打开 session 的内嵌终端。
- 前端：Svelte 5 + TypeScript + Vite。
- 主机管理：界面里添加、删除、编辑，并可从 `~/.ssh/config` 勾选导入。
- 建立六个脚本（S5）：`debug-desktop`、`release`、`install-local`，各有
  `.sh` 与 `.ps1`。

### 1.2 假设（可纠正）

- 平台优先级：Windows、macOS；Linux 桌面不是目标，但 CI 在 Ubuntu 上仍编译
  `mai-app`（安装 webkit2gtk 依赖）。
- 包管理器：pnpm（本机已有 pnpm 10、Node 24、tauri-cli 2）。
- 界面文案为中文，放在资源文件 `ui/src/i18n/zh.json`（总设计第 14 节）；
  Rust 侧只输出 ASCII。

### 1.3 不在 04a

agent 列表与状态、提醒（应用内、系统通知、声音）、点击跳转、指标栏、设置页、
状态持久化（S6）、首次连接提醒抑制（S13）、清理功能（S8）及其他 S 项。
04a 转发 `Update` 时忽略 `Agent`、`Alert`、`Panes`、`Metrics`、`Hooks`、
`Dropped`。

## 2. 结构

```text
crates/mai-app/                 Tauri 2 应用（工作区成员）
  Cargo.toml、tauri.conf.json、build.rs、capabilities/
  src/main.rs                   入口：读配置、启动 HostManager、注册命令
  src/config.rs                 config.toml 读写（原子写入）
  src/dto.rs                    Update -> 可序列化 DTO
  src/prompt.rs                 Prompter 实现：事件 + oneshot 应答
  src/terminal.rs               终端转发：批量合并、有界缓冲、Channel
  src/import.rs                 ~/.ssh/config Host 别名列表
  src/commands.rs               Tauri 命令
  ui/                           Svelte 5 前端（pnpm、Vite）
    src/lib/stores/             主机、终端标签、对话框状态
    src/lib/components/         主机树、标签栏、终端、状态栏、对话框
    src/i18n/zh.json            界面文案
scripts/                        六个脚本
```

`mai-core` 仍与界面无关；`mai-app` 只做桥接。

## 3. Rust 与前端的桥接

采用 Tauri 命令 + 事件 + Channel：

- 命令：`snapshot`、`list_hosts`、`add_host`、`update_host`、`remove_host`、
  `retry_host`、`import_ssh_hosts`、`open_terminal`、`write_terminal`、
  `resize_terminal`、`close_terminal`、`answer_prompt`。
- 状态：一个转发任务把 `mai-core` 的 `Update` 转成 DTO，以 `update` 事件发出；
  前端加载后先调用 `snapshot` 取当前全部主机状态，再接收增量。
- 终端：`open_terminal` 为每个终端注册一个 `tauri::ipc::Channel`；输出约 16 ms
  或 64 KB 合并一批发送（S15），每个终端的待发缓冲有上限（1 MB）；超出时丢弃
  全部积压（只丢一部分会切断转义序列），向前端发一条“重绘”控制消息（前端清屏），
  并把终端尺寸先改小一列再改回，让 zellij 重绘整屏（S16 的应用侧部分）；
  `Attached`、`Detached`、
  `Exited` 作为控制消息走同一 Channel。输入、尺寸、关闭走命令。

## 4. 交互提示（Prompter）

`mai-app` 实现 `mai-core` 的 `Prompter`：

- 每次提示生成 id，发 `prompt` 事件（类型：主机密钥、密码、口令、
  keyboard-interactive；附主机、用户、指纹或问题），用 oneshot 等待
  `answer_prompt(id, answer)`。
- 取消、关闭窗口、应用退出均视为拒绝（`None` / `false`）。
- 密码与口令对话框有“记住到钥匙串”勾选，对应 `Secret.remember`；钥匙串用真实的
  `KeyringStore`。
- 主机密钥变化、被吊销不是对话框，而是状态栏错误（总设计 3.3，无“忽略”按钮）。

## 5. 配置与数据

- 位置：总设计第 10 节的配置目录（macOS `~/Library/Application Support/multi-ai/`，
  Windows `%APPDATA%\multi-ai\`，Linux `~/.config/multi-ai/`）。
- `config.toml`：

  ```toml
  client_id = "k3f9..."          # 首次启动生成，[A-Za-z0-9_-]
  [[hosts]]
  id = "ubuntu"                  # 稳定、唯一，[A-Za-z0-9_.-]
  name = "Ubuntu"                # 显示名
  target = "ubuntu@100.66.61.30" # ssh 别名或 [user@]host[:port]
  zellij = "/opt/zellij"         # 可选
  ```

- 本机主机 `local` 不写入文件、始终存在、不可删除（可设置 zellij 路径，写在
  `[local]` 表）。
- `id` 由显示名或目标生成（非法字符替换为 `-`，重复时加 `-2`、`-3`）。编辑目标或
  zellij 路径 = 以同一 id 移除后重新加入（03e 的代数机制保证旧事件被丢弃）。
- 写入：临时文件 + 重命名。解析失败：拒绝启动，以对话框显示文件路径与行号，
  不用默认值覆盖（总设计第 11 节）。
- 应用自己的 known_hosts：配置目录下的 `known_hosts`；同时读取
  `~/.ssh/known_hosts`。
- 探针二进制：发布版放在应用资源目录 `probes/<target>/`；调试时取仓库根目录的
  `probes/`（可用环境变量 `MAI_PROBES` 覆盖）。

## 6. 界面

```text
+-------------------+------------------------------------------+
| 主机         [+]  | [ubuntu:work] [local:dev*]               |
| (o) local         |------------------------------------------|
|     dev           |                                          |
|     work          |           xterm.js（zellij attach）       |
| (o) ubuntu        |                                          |
|     work          |                                          |
| (!) mac           |                                          |
+-------------------+------------------------------------------+
| ubuntu: 已连接 | 提示：ssh config 第 3 行 Match 被忽略 | [重试]  |
+--------------------------------------------------------------+
```

- 左侧主机树：每台主机一个状态点（已连接、部分可用、连接中、离线、需处理）；
  展开列出探针报告的 zellij session（已退出的置灰）。右键菜单：编辑、删除、
  立即重试、新建 session。顶部“+”添加主机，菜单中可“从 ssh config 导入”。
- 顶部标签：每个 `(host, session)` 一个；点击已打开的 session 只切换。标题
  `host:session`；断开时置灰并显示“重连中”；关闭标签只 detach（session 保留，
  B33）；zellij 退出时显示“session 已结束”与关闭按钮。
- 主区域：xterm.js，加载 `@xterm/addon-clipboard`、`@xterm/addon-unicode11`、
  `@xterm/addon-fit`；跟随窗口尺寸调用 `resize_terminal`。光标位置查询（S14）
  由 xterm.js 应答，应用不代答。
- 底部状态栏：当前主机的状态与重试原因或问题（红色）、“重试”按钮；当前提示
  （notes）：收到 `Probe(Connecting)` 时清除，收到新提示时替换（S18）。
- 对话框：添加或编辑主机（显示名、目标、可选 zellij 路径）；从 ssh config 导入
  （Host 别名勾选列表，跳过含 `*`、`?` 的模式，已存在的标出）；主机密钥确认
  （指纹，信任或拒绝）；密码、口令、keyboard-interactive（逐项输入，可勾选记住）；
  新建 session（名称，空名或以 `-` 开头拒绝）。
- 终端连接状态（S17）：`open_terminal` 立即返回，标签先显示“连接中”，收到
  `Attached` 后显示画面。

## 7. 对 mai-core 的修改

- S17：终端任务开始连接前发出 `HostEvent::Term(Some(ConnState::Connecting))`。
- S10：`Update` 接收端被丢弃后，监视任务结束，并停止所有主机任务（不再继续连接、
  确认 hook 事件）。
- S16 只在应用侧处理（第 3 节）；`mai-core` 内部的无界通道作为已知限制记录。

## 8. 错误处理

- 命令返回 `Result<T, String>`，前端在对话框或状态栏显示。
- 连接问题不作为命令错误，而经 `update` 体现（总设计 3.4）。
- 配置解析失败：见第 5 节。

## 9. 测试

- Rust（`mai-app`）单元测试：`config.toml` 往返、坏文件、缺字段；id 生成与去重；
  DTO 转换；提示的应答、取消、窗口关闭；输出批量合并、上限、刷新；ssh config
  导入（别名列表、跳过通配）。
- `mai-core`：S17、S10 各有测试。
- 前端：vitest 测 store（应用 update、标签状态、表单校验）；`svelte-check` 与
  `tsc` 零警告。
- CI：Rust 测试三平台（Ubuntu 安装 webkit2gtk 等依赖）；前端在 Ubuntu 上
  安装依赖、检查、测试、构建一次。
- 端到端（手工）：Windows 本机用 `scripts/debug-desktop.ps1` 启动；添加本机与
  Ubuntu；新建一次性 session；输入、缩放；关闭标签后 session 仍在；结束时清理
  该 session。Mac 仅在用户同意后。

## 10. 六个脚本（S5）

- `debug-desktop.sh` / `.ps1`：构建本机探针到 `probes/<target>/`，安装前端依赖，
  `pnpm tauri dev`。
- `release.sh` / `.ps1`：检查 `probes/` 含全部 5 个目标（来自 CI 产物，
  `gh run download --name probes --dir probes`），`pnpm tauri build`。
- `install-local.sh` / `.ps1`：安装发布产物供发布前验证。Windows 静默运行 NSIS
  安装包；macOS 复制 `.app` 到 `~/Applications`；Linux 复制可执行文件到
  `~/.local/bin`。

脚本与用途写入 README。

## 11. 已知限制

- `mai-core` 终端输出通道无界（S16 核心部分）。
- 同一台机器的两条主机记录会互相 stop 对方的 serve（S12），04a 只在文档中说明。
