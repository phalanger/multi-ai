# multi-ai

Monitor AI coding agents (Claude Code, Codex, cmagent, ...) running in
zellij sessions across SSH hosts.

Design: `docs/superpowers/specs/2026-09-23-multi-ai-monitor-design.md`

## Crates

| crate | purpose |
| --- | --- |
| `mai-protocol` | probe/app wire messages (JSON Lines) |
| `mai-core` | UI-independent core: agent state tracking and alerts |

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
