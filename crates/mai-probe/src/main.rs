//! mai-probe: remote probe for the multi-ai app.
//!
//! `serve` is started by the app over SSH and speaks JSON Lines on
//! stdin/stdout. `hook` and `emit` are called by agents and only append to
//! the spool, so they never slow an agent down.

use std::env;
use std::io::{self, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use mai_probe::install::{
    CLAUDE_EVENTS, CODEX_EVENTS, Outcome, hook_command, install_file, uninstall_file,
};
use mai_probe::rules::{default_rules, parse_rules};
use mai_probe::run::{now_ms, run};
use mai_probe::serve::Server;
use mai_probe::spool::{Spool, SpoolRecord};
use mai_probe::zellij::{CliZellij, find_via_login_shell, find_zellij};
use mai_protocol::{AgentState, PROTOCOL_VERSION, ProbeMsg};
use serde_json::{Value, json};

const SPOOL_KEEP_DAYS: u64 = 7;

#[derive(Parser)]
#[command(name = "mai-probe", version, about = "multi-ai remote probe")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Stream agent events and metrics over stdin/stdout (started by the app).
    Serve {
        /// zellij binary; default: PATH, common install dirs, login shell.
        #[arg(long)]
        zellij: Option<PathBuf>,
        /// Scrape rules TOML; default: built-in rules.
        #[arg(long)]
        rules: Option<PathBuf>,
    },
    /// Called by agent hooks: record one event (JSON payload on stdin).
    Hook { agent: String },
    /// Report a state for any agent (e.g. cmagent) from inside its pane.
    Emit {
        #[arg(long)]
        agent: String,
        /// unknown | working | needs_input | done | exited
        #[arg(long)]
        state: String,
        #[arg(long)]
        msg: Option<String>,
    },
    /// Add our hooks to Claude Code and Codex configs.
    InstallHooks,
    /// Remove our hooks from Claude Code and Codex configs.
    UninstallHooks,
}

fn home_dir() -> PathBuf {
    let (first, second) = if cfg!(windows) {
        ("USERPROFILE", "HOME")
    } else {
        ("HOME", "USERPROFILE")
    };
    env::var_os(first)
        .or_else(|| env::var_os(second))
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// Probe data dir: `$MAI_HOME` if set, else `<home>/.mai`.
fn spool(home: &Path) -> Spool {
    let mai = env::var_os("MAI_HOME").map_or_else(|| home.join(".mai"), PathBuf::from);
    Spool::new(mai.join("spool"))
}

/// Spool record for the zellij pane this process runs in, if any.
fn record_here(agent: &str, payload: Value) -> Option<SpoolRecord> {
    let session = env::var("ZELLIJ_SESSION_NAME").ok()?;
    let pane_id = env::var("ZELLIJ_PANE_ID").ok()?.parse().ok()?;
    Some(SpoolRecord {
        ts_ms: now_ms(),
        agent: agent.to_owned(),
        session,
        pane_id,
        payload,
    })
}

/// Never fails the agent: problems go to stderr, exit code is always 0.
/// Prints nothing on stdout (Claude Code feeds hook stdout to the model).
fn cmd_hook(home: &Path, agent: &str) {
    let mut text = String::new();
    if let Err(e) = io::stdin().read_to_string(&mut text) {
        return eprintln!("mai-probe hook: read stdin: {e}");
    }
    // Some Windows shells prefix piped text with a UTF-8 BOM.
    let payload: Value = match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
        Ok(v) => v,
        Err(e) => return eprintln!("mai-probe hook: payload is not JSON: {e}"),
    };
    let Some(rec) = record_here(agent, payload) else {
        return; // not inside a zellij pane: nothing to track
    };
    if let Err(e) = spool(home).append(&rec) {
        eprintln!("mai-probe hook: spool: {e}");
    }
}

fn cmd_emit(home: &Path, agent: &str, state: &str, msg: Option<String>) -> ExitCode {
    if serde_json::from_value::<AgentState>(json!(state)).is_err() {
        eprintln!("mai-probe emit: unknown state '{state}'");
        return ExitCode::from(2);
    }
    let Some(rec) = record_here(agent, json!({"state": state, "message": msg})) else {
        eprintln!("mai-probe emit: not inside a zellij pane (ZELLIJ_PANE_ID unset)");
        return ExitCode::FAILURE;
    };
    match spool(home).append(&rec) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("mai-probe emit: spool: {e}");
            ExitCode::FAILURE
        }
    }
}

/// One JSON line per agent on stdout; exit 1 if any agent failed.
fn cmd_hooks(home: &Path, install: bool) -> ExitCode {
    let exe = match env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("mai-probe: cannot locate own executable: {e}");
            return ExitCode::FAILURE;
        }
    };
    let targets = [
        (
            "claude",
            home.join(".claude"),
            "settings.json",
            CLAUDE_EVENTS,
        ),
        ("codex", home.join(".codex"), "hooks.json", CODEX_EVENTS),
    ];
    let mut ok = true;
    for (agent, dir, file, events) in targets {
        let path = dir.join(file);
        let line = if !dir.is_dir() {
            json!({"agent": agent, "outcome": "skipped", "reason": "agent config dir not found"})
        } else {
            let result = if install {
                install_file(&path, events, &hook_command(&exe, agent), now_ms())
            } else {
                uninstall_file(&path, now_ms())
            };
            match result {
                Ok(outcome) => {
                    let mut line = json!({
                        "agent": agent,
                        "outcome": format!("{outcome:?}").to_lowercase(),
                        "path": path.display().to_string(),
                    });
                    if install && agent == "codex" && outcome != Outcome::Unchanged {
                        line["note"] = json!(
                            "Codex runs these hooks only after you trust them: run /hooks in Codex"
                        );
                    }
                    line
                }
                Err(e) => {
                    ok = false;
                    json!({"agent": agent, "outcome": "error", "error": e.to_string()})
                }
            }
        };
        println!("{line}");
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn cmd_serve(home: &Path, zellij: Option<PathBuf>, rules: Option<PathBuf>) -> io::Result<()> {
    let rules = match rules {
        None => default_rules(),
        Some(p) => parse_rules(&std::fs::read_to_string(&p)?).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", p.display()))
        })?,
    };
    let exe = find_zellij(zellij.as_deref(), env::var_os("PATH").as_deref(), home).or_else(|| {
        let shell = env::var_os("SHELL").filter(|_| !cfg!(windows))?;
        find_via_login_shell(&shell)
    });
    let cli = exe.clone().map(CliZellij::new);
    let hello = ProbeMsg::Hello {
        protocol_version: PROTOCOL_VERSION,
        probe_version: env!("CARGO_PKG_VERSION").to_owned(),
        os: env::consts::OS.to_owned(),
        arch: env::consts::ARCH.to_owned(),
        zellij_path: exe.map(|p| p.display().to_string()),
        zellij_version: cli.as_ref().and_then(|c| c.version().ok()),
    };
    let spool = spool(home);
    if let Err(e) = spool.cleanup(now_ms(), SPOOL_KEEP_DAYS) {
        eprintln!("mai-probe serve: spool cleanup: {e}");
    }
    let server = Server::new(cli, spool, &rules)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    run(
        server,
        hello,
        BufReader::new(io::stdin()),
        io::stdout().lock(),
    )
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let home = home_dir();
    match cli.cmd {
        Cmd::Serve { zellij, rules } => match cmd_serve(&home, zellij, rules) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("mai-probe serve: {e}");
                ExitCode::FAILURE
            }
        },
        Cmd::Hook { agent } => {
            cmd_hook(&home, &agent);
            ExitCode::SUCCESS
        }
        Cmd::Emit { agent, state, msg } => cmd_emit(&home, &agent, &state, msg),
        Cmd::InstallHooks => cmd_hooks(&home, true),
        Cmd::UninstallHooks => cmd_hooks(&home, false),
    }
}
