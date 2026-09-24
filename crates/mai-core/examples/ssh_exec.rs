//! Manual check: connect to a host and run one command.
//!
//! cargo run -p mai-core --example ssh_exec -- <alias|user@host[:port]> <command>
//!
//! See `common/mod.rs` for environment variables and prompt behavior.

#[path = "common/mod.rs"]
mod common;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [target, command] = args.as_slice() else {
        eprintln!("usage: ssh_exec <alias|user@host[:port]> <command>");
        std::process::exit(2);
    };
    let session = common::connect_or_exit(target).await;
    let out = session.exec(command).await.expect("exec");
    println!("status: {:?}", out.status);
    println!("stdout: {}", out.stdout_str());
    println!("stderr: {}", String::from_utf8_lossy(&out.stderr));
    session.close().await;
}
