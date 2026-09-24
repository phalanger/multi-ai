# multi-ai

Monitor AI coding agents (Claude Code, Codex, cmagent, ...) running in
zellij sessions across SSH hosts.

Design: `docs/superpowers/specs/2026-09-23-multi-ai-monitor-design.md`

## Crates

| crate | purpose |
| --- | --- |
| `mai-protocol` | probe/app wire messages (JSON Lines) |
| `mai-core` | UI-independent core: SSH, probe deploy, agent state tracking |
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

Manual checks (prompts on the terminal, secrets kept in memory):

```bash
cargo run -p mai-core --example ssh_exec -- <target> '<command>'
cargo run -p mai-core --example deploy_probe -- <target> <probes-dir> [.mai-e2e] [seconds]
```

Probe binaries for all targets come from CI:
`gh run download --name probes --dir probes`.

## mai-probe

```bash
mai-probe serve [--zellij PATH] [--rules FILE]   # JSON Lines on stdin/stdout
mai-probe hook <agent>                           # called by agent hooks
mai-probe emit --agent A --state S [--msg M]     # report state from any agent
mai-probe install-hooks | uninstall-hooks
```

`install-hooks` requires the probe to run from `~/.mai/bin` (absolute
path): hook entries are recognised by that path, so it refuses otherwise
and writes nothing. `install-hooks` / `uninstall-hooks` print one JSON
line per agent (`claude`, `codex`) with `outcome`
`installed|removed|unchanged|skipped|error`.

Data dir: `$MAI_HOME`, default `~/.mai` (spool in `spool/`).
Default scrape rules: `crates/mai-probe/rules/default.toml`.
