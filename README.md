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
  using `~/.ssh/config` (HostName, User, Port, IdentityFile, ProxyJump).
- `ssh::client::connect` authenticates with key files, ssh-agent, remembered
  or prompted password, then keyboard-interactive; unknown host keys are
  confirmed through `Prompter`, changed keys are refused.
- `deploy::deploy` uploads the matching probe (skipped when SHA-256 matches)
  and runs `install-hooks`.
- `manager::HostManager` runs one task per host (`host::run_host`): connect,
  deploy, start `serve`, relay messages, reconnect with backoff (1 s doubling
  to 60 s). Problems that need the user (authentication, host keys, deploy,
  config, protocol) wait for `retry`. `monitor::Monitor` merges all hosts
  into `Update`s (host state, sessions, panes, agents, alerts, metrics).
- `connect::SystemConnector` reaches SSH hosts (probe on an exec channel) and
  the local machine (probe as a child process, no sshd needed).

Manual checks (prompts on the terminal, secrets kept in memory):

```bash
cargo run -p mai-core --example ssh_exec -- <target> '<command>'
cargo run -p mai-core --example deploy_probe -- <target> <probes-dir> [.mai-e2e] [seconds]
cargo run -p mai-core --example monitor -- <probes-dir> <seconds> <host>...  # host: local | ssh target
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
