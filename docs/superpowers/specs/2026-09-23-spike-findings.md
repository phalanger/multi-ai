# Spike 结论

- 日期：2026-09-23
- 状态：U1-U5 已完成（Safari 未验证）
- 环境：本机 Windows 11，zellij 0.44.3（原生），Claude Code 2.1.281，
  Codex 0.155.1；Linux 主机 Ubuntu（Tailscale `100.66.61.30`，延迟约 380ms）
- 样本数据：`docs/superpowers/spike-data/`

## U1 zellij 多客户端焦点

- 观察：
  - 在 session A 内执行 `zellij --session B action ...` 可正确作用于 B。
  - `new-pane` 会把调用者的环境变量传给新 pane
    （Claude 提示继承了 `CLAUDE_CODE_CHILD_SESSION`）。
  - 用命令启动的 pane，`list-panes -c` 的 `terminal_command` 为该命令；
    默认 shell 的 pane 为 null。
  - `list-clients` 输出每个客户端的 `CLIENT_ID` 与当前焦点 pane，
    可用于程序化验证。
  - 两个客户端同时 attach 时，外部执行的 `focus-pane-id`、
    `go-to-tab-name` **只作用于最近有输入的客户端**：
    客户端 2 按键后，外部跳转改为作用于客户端 2，客户端 1 不变。
  - macOS（zellij 0.45.1）经 SSH exec（无 TTY）：
    `attach --create-background`、`--session X action new-pane`、
    `list-panes -j`、`dump-screen -p` 均正常。
- 对设计的影响：
  - 设计 8.1 的“探针执行 `Focus`”只在应用自己的客户端是最近活跃
    客户端时正确。用户同时在别的终端 attach 同一 session 并有输入时，
    跳转会作用到那个终端。
  - 04-app 需补实验：应用终端获得焦点时，能否让自己的客户端成为
    最近活跃客户端（例如 xterm 焦点事件、窗口尺寸变化是否计为活跃）；
    不行则回退到设计 8.2 的按键序列方案。

## U2 Windows 远程 ConPTY + zellij attach

- 通过：russh 连本机 Windows OpenSSH（密码认证），PTY 中运行
  `zellij attach -c`，界面、输入、尺寸变化均正常（见 U5 的 C1-C8）。
- 注意：本机 sshd 对管理员账号只读
  `C:\ProgramData\ssh\administrators_authorized_keys`，
  普通 `~/.ssh/authorized_keys` 中的公钥不生效。
- 同一 Windows 主机上，经 sshd 启动的 zellij server 运行在服务会话中。

## U3 Claude Code hook 事件

事件表（样本：`spike-data/claude-hooks.jsonl`）：

| 事件 | 触发时机 | 映射状态 |
| --- | --- | --- |
| `SessionStart` | 启动 | Unknown |
| `UserPromptSubmit` | 提交提示词 | Working |
| `PreToolUse` / `PostToolUse` | 工具调用前后 | Working |
| `Notification` `permission_prompt` | 弹出授权确认 | NeedsInput |
| `Stop` | 一轮结束 | Done |
| `Notification` `idle_prompt` | Stop 后约 60 秒 | Done（不重复提醒） |
| `SessionEnd` | 退出，`reason` 如 `prompt_input_exit` | Exited |

- 公共字段：`session_id`、`transcript_path`、`cwd`、`hook_event_name`；
  多数事件带 `prompt_id`、`permission_mode`。
- `Notification` 用 `notification_type` 区分授权与空闲，`message` 为提示文本。
- hook 进程环境中有 `ZELLIJ_SESSION_NAME`、`ZELLIJ_PANE_ID`，可精确定位 pane。
- 默认权限模式已是 auto，大部分授权会被自动处理；
  NeedsInput 只在 manual 等模式或高风险操作时出现。
- 退出后 pane 变为 `exited: true, exit_status: 0, is_held: true`，
  无 hook 也能检测退出。

## U4 Codex notify / hooks

- Codex 0.155.1 的 `hooks` 为 stable 且默认启用，格式与 Claude Code 基本一致。
  配置位置：`~/.codex/hooks.json`、`~/.codex/config.toml`、
  项目级 `.codex/hooks.json`；**不能**用 `-c` 设置。
- 非托管 hooks 需信任：交互 `/hooks` 审核，或 `--dangerously-bypass-hook-trust`。
  项目目录还需被信任（写入全局 `config.toml` 的 `[projects.'<小写盘符路径>']`）。

事件表（样本：`spike-data/codex-events.jsonl`）：

| 事件 | 触发时机 | 映射状态 |
| --- | --- | --- |
| `SessionStart` | 第一次提交提示词时（非启动时） | Unknown |
| `UserPromptSubmit` / `PreToolUse` | 提交、工具调用前 | Working |
| `PermissionRequest` | 弹出审批 | NeedsInput |
| `Stop` | 一轮结束 | Done |
| `Interrupt` | 用户按 Esc 拒绝或中断 | Done（空闲等待输入） |
| `SessionEnd` | `/quit` 退出 | Exited |

- `notify`（`agent-turn-complete`）**有误报**：Codex 内部生成任务标题的
  轮次也会触发，`thread-id` 不同。结论：以 hooks 为主，不用 notify。

## U5 xterm.js 渲染 zellij

- 环境：termbridge（russh + axum WebSocket + xterm.js 5 CDN），
  本机 Windows sshd，Edge 浏览器（与 WebView2 同内核）。
- 结果：C1-C8 全部通过（界面、缩放重排、微软拼音输入中文、
  中文与 emoji 宽字符对齐、鼠标、模式键与 Alt 组合键、复制粘贴、大量输出）。
- C7 复制：zellij 复制走 OSC 52。xterm.js 默认不处理 OSC 52，
  复制无效；加载 `@xterm/addon-clipboard` 后可写入系统剪贴板。
  正式应用必须加载该插件（或在 Tauri 侧接管剪贴板写入）。
- Safari / WKWebView：未验证（spike 仅在 Windows 浏览器上测试）。
- 远程 macOS（zellij 0.45.1，经 Tailscale）复测 C1、C3、C4：
  - C1 通过。
  - C3 初测失败：russh 未转发任何环境变量，SSH exec 下
    `LC_CTYPE="C"`，中文输入异常。命令前加
    `LANG=en_US.UTF-8 LC_CTYPE=en_US.UTF-8` 并新建 session 后通过。
    注意 zellij server 创建时的 locale 会被其中的 shell 继承，
    已在 C locale 下创建的 session 需重建。
  - C4 初测失败：`🙂` 与后一个字符重叠。原因是 xterm.js 默认使用
    Unicode 6 宽度表（`🙂` 占 1 格），zellij 按 2 格排版。
    加载 `@xterm/addon-unicode11` 并设 `term.unicode.activeVersion = '11'`
    （需 `allowProposedApi: true`）后通过；emoji 字形略显拥挤，
    可在 04-app 试 `rescaleOverlappingGlyphs` 或调整字体。
  - 本机 Windows 上 C4 未出现该问题，推测 ConPTY 对输出做了重排。

## 抓屏规则素材

- Claude：工作中为 spinner 行（如 `✶ Ionizing… (3s · ↓ 2 tokens)`），
  完成为 `✻ Cooked for 3s · done 19:48`，授权为 `Do you want to proceed?`。
- Codex：工作中为 `• Working (2s • esc to interrupt)`，完成为 `done 7:54 PM`，
  审批为 `Would you like to run the following command?`。
- **空闲时屏幕持续变化**：Claude 的状态栏插件有每分钟变化的倒计时；
  Codex 空闲时有盲文点阵背景动画。纯“屏幕哈希”判断对两者都失效。

## 其他发现

- macOS 非交互 SSH exec 的 PATH 为
  `/Users/cyt/.cargo/bin:/usr/bin:/bin:/usr/sbin:/sbin`，
  不含 `/opt/homebrew/bin`；`zsh -lc` 能找到 zellij，但找不到 claude、codex。
  （证实 U6：需每主机可配置 zellij 路径，探针应检查常见位置。）
- 各主机 zellij 版本不同（本机 0.44.3，Mac 0.45.1），
  探针须按 `Hello` 报告的版本兼容 CLI 差异。

- Windows 上在无控制台的进程中 `zellij attach --create-background`
  启动的 session 会立即退出（日志：`Failed to run command: program not found`
  后 server 退出）。应用通过 PTY attach，不依赖此功能。
- Git Bash（MSYS）会把以 `/` 开头的参数改写为 Windows 路径
  （`/exit` 变为 `D:/Program Files/Git/exit`）；需 `MSYS_NO_PATHCONV=1`。
- russh 0.63.3 默认 `aws-lc-rs` 后端在 Windows 编译需 NASM；
  改用 `default-features = false, features = ["flate2", "ring", "rsa"]` 可免。
  `Handler::check_server_key` 参数类型为 `&russh::keys::PublicKeyOrCertificate`。
- spike 计划中 `spikes/*` 下的 crate 需在 `Cargo.toml` 加空的 `[workspace]`，
  否则被仓库根 workspace 拒绝。

## 对用户环境的改动与恢复

- Codex 项目信任：spike 期间全局 `~/.codex/config.toml` 被加入
  `[projects.'g:\work\ai\multi-ai\spikes\codex-playground']`，
  结束后已按行删除；Codex 自身维护的提示计数器变化保留。
- 测试 zellij session（本机 `mai-spike-claude`、`mai-spike-u5`，
  Mac `mai-spike`、`mai-spike2`）均已删除；`work` session 未改动。
- Claude hook 配置只写在 `spikes/` 下的项目级文件，未改全局配置。

## 设计变更清单

1. 状态检测：Claude 与 Codex 都以 hooks 为主；弃用 Codex `notify`。
2. 抓屏兜底：规则需支持“忽略区域/行”（状态栏、动画行），
   或改为只匹配关键行，不再以整屏哈希判定 Working。
3. hook 安装：Codex 需写 `hooks.json`，并处理 hook 信任与项目信任。
4. 03-core：russh 使用 `ring` 后端。
5. 04-app：xterm.js 必须加载 `@xterm/addon-clipboard`（OSC 52）与
   `@xterm/addon-unicode11`（宽字符宽度与 zellij 一致）。
6. 03-core：远程命令必须带 UTF-8 locale：优先通过 SSH env 请求
   发送 `LANG`/`LC_CTYPE`（macOS sshd 默认 `AcceptEnv LANG LC_*`），
   服务器拒绝时在命令前加 `LANG=... LC_CTYPE=...`。
7. 跳转：外部 `action` 作用于最近活跃客户端；04-app 需先做焦点实验，
   不可靠时采用按键序列方案（设计 8.2）。
8. 探针：查找 zellij 时除 PATH 外检查常见安装位置
   （如 `/opt/homebrew/bin`、`~/.cargo/bin`），仍找不到才提示用户。
