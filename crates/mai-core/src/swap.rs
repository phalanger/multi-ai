//! Put a freshly written probe binary in place without deleting the old
//! one first (B19). On Windows a running executable cannot be deleted but
//! can be renamed, so the old binary is moved aside to `<exe>.old` (or
//! `<exe>.old-<stamp>` when an older `.old` is still locked), the new one
//! is renamed into place, and the old one is removed when possible.
//! Leftover `<exe>.old*` files are removed on the next swap.

use std::future::Future;
use std::io;

/// The file operations a swap needs, on `/`-separated paths.
pub trait BinFiles: Sync {
    fn exists(&self, path: &str) -> impl Future<Output = io::Result<bool>> + Send;
    fn remove(&self, path: &str) -> impl Future<Output = io::Result<()>> + Send;
    fn rename(&self, from: &str, to: &str) -> impl Future<Output = io::Result<()>> + Send;
    /// File names in `dir`.
    fn list(&self, dir: &str) -> impl Future<Output = io::Result<Vec<String>>> + Send;
}

/// Replace `<dir>/<exe>` with `<dir>/<exe>.upload`. `stamp` makes the
/// aside name unique when needed (e.g. milliseconds since the epoch).
pub async fn swap_in<F: BinFiles>(files: &F, dir: &str, exe: &str, stamp: u64) -> io::Result<()> {
    let target = format!("{dir}/{exe}");
    let tmp = format!("{target}.upload");
    let old_prefix = format!("{exe}.old");
    // Best effort: leftovers of earlier swaps (still locked ones stay).
    if let Ok(names) = files.list(dir).await {
        for name in names.iter().filter(|n| n.starts_with(&old_prefix)) {
            let _ = files.remove(&format!("{dir}/{name}")).await;
        }
    }
    let mut aside = None;
    if files.exists(&target).await? {
        let mut name = format!("{target}.old");
        if files.exists(&name).await? {
            name = format!("{target}.old-{stamp}");
        }
        files.rename(&target, &name).await?;
        aside = Some(name);
    }
    if let Err(e) = files.rename(&tmp, &target).await {
        // Put the old binary back so the host keeps a working probe.
        if let Some(name) = &aside
            && let Err(re) = files.rename(name, &target).await
        {
            return Err(io::Error::new(
                e.kind(),
                format!("{e}; restoring {name} also failed: {re}"),
            ));
        }
        return Err(e);
    }
    if let Some(name) = aside {
        let _ = files.remove(&name).await;
    }
    Ok(())
}

/// `BinFiles` on the local file system.
pub struct LocalFiles;

impl BinFiles for LocalFiles {
    async fn exists(&self, path: &str) -> io::Result<bool> {
        tokio::fs::try_exists(path).await
    }

    async fn remove(&self, path: &str) -> io::Result<()> {
        tokio::fs::remove_file(path).await
    }

    async fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        tokio::fs::rename(from, to).await
    }

    async fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(e) = entries.next_entry().await? {
            names.push(e.file_name().to_string_lossy().into_owned());
        }
        Ok(names)
    }
}

/// `BinFiles` over SFTP (paths relative to the login directory).
pub struct SftpFiles<'a>(pub &'a russh_sftp::client::SftpSession);

fn sftp_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

impl BinFiles for SftpFiles<'_> {
    async fn exists(&self, path: &str) -> io::Result<bool> {
        self.0.try_exists(path.to_owned()).await.map_err(sftp_err)
    }

    async fn remove(&self, path: &str) -> io::Result<()> {
        self.0.remove_file(path.to_owned()).await.map_err(sftp_err)
    }

    async fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        self.0
            .rename(from.to_owned(), to.to_owned())
            .await
            .map_err(sftp_err)
    }

    async fn list(&self, dir: &str) -> io::Result<Vec<String>> {
        let entries = self.0.read_dir(dir.to_owned()).await.map_err(sftp_err)?;
        Ok(entries.map(|e| e.file_name()).collect())
    }
}
