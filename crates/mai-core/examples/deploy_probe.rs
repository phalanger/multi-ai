//! Manual check: deploy mai-probe to a host, then stream `serve` output.
//!
//! cargo run -p mai-core --example deploy_probe -- <target> <probes-dir> [dir] [seconds]
//!
//! `dir` defaults to `.mai-e2e` and hooks are NOT installed, so the host's
//! real `~/.mai`, `~/.claude` and `~/.codex` are untouched. Pass `.mai` to
//! test the production location (hooks are still not installed).
//! See `common/mod.rs` for environment variables and prompt behavior.

#[path = "common/mod.rs"]
mod common;

use std::path::PathBuf;
use std::time::Duration;

use mai_core::deploy::{DeployOptions, ProbeStore, deploy};
use russh::ChannelMsg;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (target, probes) = match args.as_slice() {
        [t, p, ..] => (t.as_str(), PathBuf::from(p)),
        _ => {
            eprintln!("usage: deploy_probe <target> <probes-dir> [dir] [seconds]");
            std::process::exit(2);
        }
    };
    let dir = args.get(2).cloned().unwrap_or_else(|| ".mai-e2e".into());
    let secs: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(6);

    let session = common::connect_or_exit(target).await;
    let opts = DeployOptions {
        dir,
        install_hooks: false,
    };
    let report = match deploy(&session, &ProbeStore { dir: probes }, &opts).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("deploy error: {e}");
            std::process::exit(1);
        }
    };
    println!("remote: {:?}", report.remote);
    println!(
        "probe: {} (uploaded: {})",
        report.probe_path, report.uploaded
    );

    let cmd = report.remote.invoke(&report.probe_path, "serve");
    let mut ch = session
        .open_exec(&cmd, &[("LANG", "en_US.UTF-8")])
        .await
        .expect("open serve");
    let mut buf = Vec::new();
    let deadline = tokio::time::sleep(Duration::from_secs(secs));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            msg = ch.wait() => match msg {
                Some(ChannelMsg::Data { data }) => buf.extend_from_slice(&data),
                Some(ChannelMsg::ExtendedData { data, .. }) => {
                    eprint!("[stderr] {}", String::from_utf8_lossy(&data));
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    println!("serve exited: {exit_status}");
                    break;
                }
                None => break,
                _ => {}
            },
        }
    }
    for line in String::from_utf8_lossy(&buf).lines() {
        let head: String = line.chars().take(150).collect();
        println!("{head}");
    }
    let _ = ch.eof().await;
    session.close().await;
}
