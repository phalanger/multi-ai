//! The real `TermTransport`s: an SSH session dedicated to terminals (a
//! PTY channel per terminal), or local PTYs for this machine.

use std::ffi::OsString;

use crate::deploy::Remote;
use crate::host::{OpenError, Problem};
use crate::pty::{PtyIo, spawn_local};
use crate::ssh::auth::Prompter;
use crate::ssh::client::SshSession;
use crate::term::{AttachSpec, LOCALE_ENV, TermTransport, attach_argv, remote_attach_command};

/// Terminal connection of one host.
pub enum SystemTerminals<P: Prompter> {
    /// Its own SSH session (the probe has another one), and what `detect`
    /// learned about the host's shell.
    Ssh {
        session: SshSession<P>,
        remote: Remote,
    },
    /// This machine: each terminal is a local PTY.
    Local,
}

/// Environment for a local `zellij attach`: a GUI app may have neither a
/// terminal type nor a UTF-8 locale in its environment.
pub fn local_env() -> Vec<(&'static str, &'static str)> {
    let mut env = vec![("TERM", "xterm-256color")];
    if !cfg!(windows) {
        env.extend(LOCALE_ENV);
    }
    env
}

impl<P: Prompter> TermTransport for SystemTerminals<P> {
    async fn attach(
        &mut self,
        zellij: Option<&str>,
        spec: &AttachSpec,
    ) -> Result<PtyIo, OpenError> {
        match self {
            Self::Ssh { session, remote } => {
                let (name, create) = (spec.session.as_str(), spec.create);
                session
                    .open_pty(spec.size, &LOCALE_ENV, |accepted| {
                        remote_attach_command(remote, zellij, name, create, accepted)
                    })
                    .await
                    .map_err(crate::connect::ssh_open_error)
            }
            Self::Local => {
                let program = OsString::from(zellij.unwrap_or("zellij"));
                let argv = attach_argv(&spec.session, spec.create);
                spawn_local(&program, &argv, &local_env(), spec.size).map_err(|e| {
                    OpenError::NeedsUser(Problem::Config(format!(
                        "cannot start {}: {e}",
                        program.to_string_lossy()
                    )))
                })
            }
        }
    }
}
