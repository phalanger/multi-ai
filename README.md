# multi-ai

Monitor AI coding agents (Claude Code, Codex, cmagent, ...) running in
zellij sessions across SSH hosts.

Design: `docs/superpowers/specs/2026-09-23-multi-ai-monitor-design.md`

## Crates

| crate | purpose |
| --- | --- |
| `mai-protocol` | probe/app wire messages (JSON Lines) |
| `mai-core` | UI-independent core: SSH, probe deploy, hosts, agent tracking |
| `mai-probe` | remote probe: hooks, spool, zellij polling, scrape, metrics |

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## mai-core

- `ssh::config::resolve` turns an alias or `user@host:port` into a `HostSpec`
  using `~/.ssh/config` (HostName, User, Port, IdentityFile, ProxyJump; a
  jump host's own ProxyJump is not followed). `Match` blocks are ignored;
  `Match` and `Include` caveats are reported by `config_warnings`.
- `ssh::client::connect` authenticates with key files (the public key is
  offered first; an encrypted key is decrypted only once the server accepts
  it), ssh-agent, remembered or prompted password, then keyboard-interactive.
  Host keys are checked against known_hosts (hashed names, globs, `@revoked`);
  unknown keys are confirmed through `Prompter`, changed or revoked keys are
  refused.
  `ConnectOptions::use_agent` turns the ssh-agent step off (tests do).
- `deploy::deploy` uploads the matching probe (skipped when SHA-256 matches),
  checks the uploaded file's SHA-256 on the host, swaps it in by moving the
  old binary aside (`swap::swap_in`, works while it runs on Windows) and runs
  `install-hooks`. Upload timeouts on slow links are retried like other
  network errors.
- `manager::HostManager` runs one task per host (`host::run_host`): connect,
  deploy, start `serve`, relay messages, reconnect with backoff (1 s doubling
  to 60 s). Problems that need the user (authentication, host keys, deploy,
  config, protocol) wait for `retry`. `monitor::Monitor` merges all hosts
  into `Update`s (host state, sessions, panes, agents, alerts, metrics,
  connect notes).
- `connect::SystemConnector` reaches SSH hosts (probe on an exec channel) and
  the local machine (probe as a child process, no sshd needed).
- `HostManager::open_terminal` attaches a terminal to a zellij session
  (`zellij attach [--create]`) in a PTY: over a per-host terminal SSH
  connection, or a local PTY (ConPTY on Windows). If the connection drops,
  terminals get `Detached` and are reattached automatically; a single closed
  channel reattaches only that terminal. Session names that are empty or
  start with `-` are refused. Without a zellij path from the host config or
  the probe, zellij is searched for on the host (PATH, common install dirs,
  login shell).

Manual checks (prompts on the terminal, secrets kept in memory):

```bash
cargo run -p mai-core --example ssh_exec -- <target> '<command>'
cargo run -p mai-core --example deploy_probe -- <target> <probes-dir> [.mai-e2e] [seconds]
cargo run -p mai-core --example monitor -- <probes-dir> <seconds> <host>...  # host: local | ssh target
cargo run -p mai-core --example term -- <probes-dir> <host> <new-session-name>
```

Probe binaries for all targets come from CI:
`gh run download --name probes --dir probes`.

## mai-probe

```bash
mai-probe serve [--zellij PATH] [--rules FILE] [--client ID]  # JSON Lines on stdin/stdout
mai-probe stop [--client ID]                     # stop that client's running serve
mai-probe hook <agent>                           # called by agent hooks
mai-probe emit --agent A --state S [--msg M]     # report state from any agent
mai-probe install-hooks | uninstall-hooks
```

`install-hooks` requires the probe to run from `~/.mai/bin` (absolute
path): hook entries are recognised by that path, so it refuses otherwise
and writes nothing. `install-hooks` / `uninstall-hooks` print one JSON
line per agent (`claude`, `codex`) with `outcome`
`installed|removed|unchanged|skipped|error`.

Data dir: `<dir>` when the probe runs from `<dir>/bin/` and `<dir>` starts
with `.mai` (e.g. `~/.mai`); otherwise `$MAI_HOME`, default `~/.mai`
(spool in `spool/`). One `serve` per `--client`: a new one stops the old
one (`serve-<client>.pid`), and each client has its own ack cursor.
Default scrape rules: `crates/mai-probe/rules/default.toml`.
