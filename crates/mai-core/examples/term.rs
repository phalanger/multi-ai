//! Manual check: attach a terminal to a throwaway zellij session through
//! `HostManager`, type a command, check its output, then quit the session.
//!
//! cargo run -p mai-core --example term -- <probes-dir> <host> <session>
//!
//! `<host>` is `local` or an ssh target. The probe is deployed to
//! `~/.mai-e2e` (hooks not installed) so it reports the zellij path; set
//! `MAI_E2E_ZELLIJ` to give the path yourself. The session is created with
//! `attach --create` and quit with Ctrl+q at the end, so use a name that
//! does not exist yet (e.g. `mai-e2e`). Never use a session you work in.

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
use mai_core::manager::{HostManager, Terminal};
use mai_core::monitor::Update;
use mai_core::pty::TermSize;
use mai_core::term::TermEvent;
use mai_core::tracker::TrackerConfig;
use tokio::time::timeout;

const MARK: &str = "MAI_E2E_OK";

/// Drop escape sequences (CSI and OSC), keep the text.
fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for d in chars.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(d) = chars.next() {
                    if d == '\u{7}' || (d == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Read terminal output for up to `wait`, answering cursor-position
/// queries (ConPTY waits for one), until `stop` matches the text so far.
async fn read_for(
    term: &mut Terminal,
    seen: &mut Vec<u8>,
    wait: Duration,
    stop: impl Fn(&str) -> bool,
) -> Option<TermEvent> {
    let deadline = tokio::time::Instant::now() + wait;
    while !stop(&strip_ansi(seen)) {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match timeout(left, term.recv()).await {
            Err(_) => return None,
            Ok(None) => return Some(TermEvent::Exited(None)),
            Ok(Some(TermEvent::Output(d))) => {
                if d.windows(4).any(|w| w == b"\x1b[6n") {
                    term.write(b"\x1b[1;1R".to_vec());
                }
                seen.extend_from_slice(&d);
            }
            Ok(Some(TermEvent::Attached)) => {}
            Ok(Some(other)) => return Some(other),
        }
    }
    None
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [probes, host, session] = args.as_slice() else {
        eprintln!("usage: term <probes-dir> <host> <session>  (host: local | ssh target)");
        std::process::exit(2);
    };
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    let home = common::home();
    let mut connector = SystemConnector::new(
        common::options(),
        Arc::new(TermPrompter),
        Arc::new(MemoryStore::default()),
        ProbeStore {
            dir: PathBuf::from(probes),
        },
        Some(
            std::env::var_os("MAI_SSH_CONFIG")
                .map_or_else(|| home.join(".ssh").join("config"), PathBuf::from),
        ),
        home,
        user,
        "e2e".to_owned(),
    );
    connector.dir = ".mai-e2e".into();
    connector.install_hooks = false;
    let (mut manager, mut updates) =
        HostManager::start(Arc::new(connector), TrackerConfig::default());
    let kind = if host == "local" {
        HostKind::Local
    } else {
        HostKind::Ssh {
            target: host.clone(),
        }
    };
    manager.add_host(HostConfig {
        id: host.clone(),
        kind,
        zellij: std::env::var("MAI_E2E_ZELLIJ").ok(),
    });

    // Wait for the probe's Hello: it reports the zellij path.
    let hello = timeout(Duration::from_secs(90), async {
        while let Some(u) = updates.recv().await {
            if let Update::Hello { info, .. } = u {
                return Some(info);
            }
        }
        None
    })
    .await;
    println!("hello: {hello:?}");

    let size = TermSize {
        cols: 100,
        rows: 30,
    };
    let mut term = match manager.open_terminal(host, session, true, size).await {
        Ok(t) => t,
        Err(e) => {
            eprintln!("open_terminal: {e}");
            std::process::exit(1);
        }
    };
    let mut seen = Vec::new();
    // Let zellij draw its UI, then type a command.
    let early = read_for(&mut term, &mut seen, Duration::from_secs(4), |_| false).await;
    println!("first event: {early:?}; {} bytes drawn", seen.len());
    // A new session may open zellij's "tips" popup; ESC dismisses it.
    term.write(vec![0x1b]);
    read_for(&mut term, &mut seen, Duration::from_secs(2), |_| false).await;
    term.write(format!("echo {MARK}\r").into_bytes());
    read_for(&mut term, &mut seen, Duration::from_secs(15), |t| {
        t.matches(MARK).count() >= 2
    })
    .await;
    let text = strip_ansi(&seen);
    let found = text.matches(MARK).count() >= 2;
    println!("command output seen: {found}");
    if !found {
        let tail: String = text
            .chars()
            .rev()
            .take(400)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        println!("output tail: {tail:?}");
    }
    term.resize(TermSize {
        cols: 120,
        rows: 40,
    });
    // Ctrl+q quits the session (zellij default keys).
    term.write(vec![0x11]);
    let end = read_for(&mut term, &mut seen, Duration::from_secs(15), |_| false).await;
    println!("after Ctrl+q: {end:?}");
    drop(term);
    drop(manager);
    tokio::time::sleep(Duration::from_millis(500)).await;
    if !found {
        std::process::exit(1);
    }
}
