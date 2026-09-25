//! Manual check: monitor hosts with `HostManager` and print every update.
//!
//! cargo run -p mai-core --example monitor -- <probes-dir> <seconds> <host>...
//!
//! A host is `local` (this machine, probe started as a child process) or
//! an ssh target. The probe goes to `~/.mai-e2e` (override with
//! `MAI_E2E_DIR`) and hooks are NOT installed, so the real `~/.mai`,
//! `~/.claude` and `~/.codex` are untouched. The client id is `e2e`.
//! `<probes-dir>` holds `<target>/mai-probe[.exe]`, e.g. the unpacked
//! `probes` CI artifact. See `common/mod.rs` for SSH-related variables.

#[path = "common/mod.rs"]
#[allow(dead_code)]
mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::{MemoryStore, TermPrompter};
use mai_core::connect::SystemConnector;
use mai_core::deploy::ProbeStore;
use mai_core::host::{HostConfig, HostKind};
use mai_core::manager::HostManager;
use mai_core::monitor::Update;
use mai_core::tracker::TrackerConfig;

fn short(u: &Update) -> String {
    let text = match u {
        Update::Panes { id, session, panes } => {
            format!("Panes {{ {id}/{session}: {} pane(s) }}", panes.len())
        }
        Update::Metrics { id, metrics } => {
            format!("Metrics {{ {id}: cpu {:.0}% }}", metrics.cpu_pct)
        }
        other => format!("{other:?}"),
    };
    text.chars().take(200).collect()
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (probes, secs, hosts) = match args.as_slice() {
        [p, s, hosts @ ..] if !hosts.is_empty() => (
            PathBuf::from(p),
            s.parse::<u64>().expect("seconds"),
            hosts.to_vec(),
        ),
        _ => {
            eprintln!(
                "usage: monitor <probes-dir> <seconds> <host>...  (host: local | ssh target)"
            );
            std::process::exit(2);
        }
    };
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let home = common::home();
    let mut connector = SystemConnector::new(
        common::options(),
        Arc::new(TermPrompter),
        Arc::new(MemoryStore::default()),
        ProbeStore { dir: probes },
        Some(
            std::env::var_os("MAI_SSH_CONFIG")
                .map_or_else(|| home.join(".ssh").join("config"), PathBuf::from),
        ),
        home,
        user,
        "e2e".to_owned(),
    );
    connector.dir = std::env::var("MAI_E2E_DIR").unwrap_or_else(|_| ".mai-e2e".into());
    connector.install_hooks = false;

    let (mut manager, mut updates) =
        HostManager::start(Arc::new(connector), TrackerConfig::default());
    for h in &hosts {
        let kind = if h == "local" {
            HostKind::Local
        } else {
            HostKind::Ssh { target: h.clone() }
        };
        manager.add_host(HostConfig {
            id: h.clone(),
            kind,
            zellij: None,
        });
    }
    let deadline = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            u = updates.recv() => match u {
                Some(u) => println!("{}", short(&u)),
                None => break,
            },
        }
    }
    drop(manager);
    // Let host tasks drop their connections (the probe exits on EOF).
    tokio::time::sleep(Duration::from_millis(500)).await;
}
