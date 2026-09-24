# multi-ai

Monitor AI coding agents (Claude Code, Codex, cmagent, ...) running in
zellij sessions across SSH hosts.

Design: `docs/superpowers/specs/2026-09-23-multi-ai-monitor-design.md`

## Crates

| crate | purpose |
| --- | --- |
| `mai-protocol` | probe/app wire messages (JSON Lines) |
| `mai-core` | UI-independent core: agent state tracking and alerts |
| `mai-probe` | remote probe: hooks, spool, zellij polling, scrape, metrics |

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## mai-probe

```bash
mai-probe serve [--zellij PATH] [--rules FILE]   # JSON Lines on stdin/stdout
mai-probe hook <agent>                           # called by agent hooks
mai-probe emit --agent A --state S [--msg M]     # report state from any agent
mai-probe install-hooks | uninstall-hooks
```

Data dir: `$MAI_HOME`, default `~/.mai` (spool in `spool/`).
Default scrape rules: `crates/mai-probe/rules/default.toml`.
