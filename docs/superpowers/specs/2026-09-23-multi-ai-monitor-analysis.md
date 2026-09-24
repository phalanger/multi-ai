# multi-ai 需求分析

- 日期：2026-09-23
- 状态：已与用户确认
- 配套设计文档：[2026-09-23-multi-ai-monitor-design.md](2026-09-23-multi-ai-monitor-design.md)

## 1. 背景

用户在多台可 SSH 到达的机器（含本机）上，于 zellij 会话中运行多个 AI 编码 agent
（Codex、Claude Code，以及自研的 cmagent，未来还会有其他 agent）。
agent 完成一轮工作或需要用户授权/回答时，用户需要及时发现并切换过去继续交互。
目前只能逐台、逐个 tab/pane 手动查看，效率低、容易遗漏。

## 2. 目标

1. 一个跨平台桌面应用（Rust 实现，首先支持 macOS 与 Windows），集中监控多台机器上的 agent。
2. 当 agent 完成工作或需要用户交互时，及时提醒用户。
3. 用户点击提醒/图标后，在应用内嵌终端中直接打开对应 zellij session 的对应 tab/pane，继续交互。
4. 后续：预定义文本一键发送（支持触摸屏）；监控主机 CPU/内存/IO/网络。

## 3. 非目标（当前阶段）

- 不替代 zellij，不实现自己的终端复用器。
- 不做多用户/团队共享，只服务单个用户。
- 不在远程主机上常驻后台服务（探针随 SSH 连接存活）。
- 推送到手机（ntfy/Bark 等）推迟到第三期。

## 4. 已确认的需求

### 4.1 主机与连接

| 编号 | 需求 |
| --- | --- |
| R-H1 | 用户在应用中录入多台主机的 SSH 连接信息，也可从 `~/.ssh/config` 导入别名 |
| R-H2 | 远程主机可为 Linux、macOS、Windows；本机也作为一台被监控主机（不走 SSH） |
| R-H3 | 每台远程主机建立两条 SSH 连接：一条用于 zellij 交互终端，一条用于探针 |
| R-H4 | 认证方式全部支持：私钥（含口令）、ssh-agent、密码、keyboard-interactive、ProxyJump |
| R-H5 | 密码与私钥口令存入系统钥匙串（macOS Keychain / Windows Credential Manager） |
| R-H6 | SSH 实现使用 russh（进程内），自行解析 `~/.ssh/config` |

### 4.2 zellij

| 编号 | 需求 |
| --- | --- |
| R-Z1 | 所有平台均为原生 zellij（用户本机为 Windows 原生 zellij 0.44.3） |
| R-Z2 | 每台主机可能有多个 zellij session，全部需要监控 |
| R-Z3 | zellij 不在 PATH 时，提示用户设置环境（或在主机设置中填写路径）后再继续，不做猜测 |

### 4.3 Agent 监控

| 编号 | 需求 |
| --- | --- |
| R-A1 | 识别 zellij pane 中运行的 agent：Claude Code、Codex、cmagent 及其他 |
| R-A2 | 判断状态：工作中、需要用户交互、本轮完成、已退出 |
| R-A3 | 检测方式：以 agent hook 上报为主，抓屏规则匹配兜底 |
| R-A4 | 允许应用自动向远程部署探针，并修改 agent 配置以安装 hook |

### 4.4 提醒

| 编号 | 需求 |
| --- | --- |
| R-N1 | 应用内视觉提醒（图标颜色、角标、置顶） |
| R-N2 | 系统通知（macOS 通知中心 / Windows Toast），点击可跳转 |
| R-N3 | 声音提醒 |
| R-N4 | 推送到手机：后续再做 |

### 4.5 交互终端

| 编号 | 需求 |
| --- | --- |
| R-T1 | 终端内嵌在应用窗口中 |
| R-T2 | 点击 agent 图标，打开对应主机 session，并定位到对应 tab/pane |
| R-T3 | 必须良好支持中文输入法与宽字符 |

### 4.6 后续需求

| 编号 | 需求 |
| --- | --- |
| R-L1 | 预定义文本，点击发送到指定 pane；支持触摸屏 |
| R-L2 | 主机性能监控：CPU、内存、磁盘 IO、网络 |

## 5. 环境事实（已在用户本机核实）

在用户本机 zellij 0.44.3（Windows 原生，路径 `d:\Apps\zellij\zellij.exe`，不在 PATH）上核实：

- `zellij action list-panes -a -j` 输出 JSON，含 pane id、tab id/name、标题、`terminal_command`、
  是否退出等字段。
- 默认 shell 的 pane，`terminal_command` 为 null，无法仅凭 pane 列表识别 agent。
- `zellij action dump-screen -p <pane_id>` 可抓取非焦点 pane 的屏幕内容。
- `zellij action focus-pane-id <pane_id>`、`go-to-tab-by-id` 可定位 pane/tab。
- `zellij action write-chars -p <pane_id>`、`paste -p <pane_id>` 可向指定 pane 发送文本。
- pane 内进程的环境变量含 `ZELLIJ_SESSION_NAME` 与 `ZELLIJ_PANE_ID`，hook 进程可直接获知所在 pane。
- zellij 0.44 自带 `zellij web`（网页服务 session），本项目不采用。

## 6. 关键风险与待验证项

| 编号 | 风险/未知 | 验证方式 |
| --- | --- | --- |
| U1 | zellij 多客户端下 `focus-pane-id` 作用于哪个客户端，能否让应用自己的客户端跳转 | spike |
| U2 | Windows 远程经 OpenSSH（ConPTY）运行 `zellij attach` 是否正常 | spike |
| U3 | Claude Code 当前 hook 事件名与载荷格式（Notification 的授权/空闲区分） | 查文档 + spike |
| U4 | Codex 当前 `notify` 事件类型，是否有开始工作/等待审批事件 | 查文档 + spike |
| U5 | xterm.js 渲染 zellij 时的中文输入法、宽字符、鼠标行为 | spike |
| U6 | 非交互 SSH exec 的 PATH 与登录 shell 不同，导致找不到 zellij/agent | 设计中处理（可配置路径） |

## 7. 方案对比结论

| 方案 | 结论 | 理由 |
| --- | --- | --- |
| Tauri 2 + xterm.js，Rust 核心 | 采用 | 中文输入与宽字符成熟；触摸 UI 成本低；自带通知插件；将来可出移动端 |
| 纯 Rust 原生 GUI（egui/iced + alacritty_terminal） | 放弃 | 终端组件中文输入法弱，触摸需自研 |
| Tauri + zellij web 客户端 | 放弃 | 需各主机起 web 服务与 token 管理，可控性差，功能较新 |
| 调用系统 ssh 可执行文件 | 放弃 | 密码/钥匙串集成别扭；选用 russh 进程内实现 |
