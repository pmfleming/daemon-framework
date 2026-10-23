use anyhow::{Context, Result, ensure};
use rustix::fs::{FlockOperation, Mode, OFlags, flock, open};
use std::{
    fs::{self, File},
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::net::{UnixListener, UnixStream};

pub struct JsonlListener {
    pub(super) listener: UnixListener,
    pub(super) uid: u32,
    path: PathBuf,
    identity: (u64, u64),
    // Never unlink the lock: replacing its inode would permit a second binder.
    _lock: File,
}
impl JsonlListener {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for JsonlListener {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|m| (m.dev(), m.ino()) == self.identity && m.file_type().is_socket())
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}
pub(super) async fn bind(runtime: &Path, name: &str) -> Result<JsonlListener> {
    ensure!(
        !name.is_empty()
            && name.len() <= 64
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "invalid socket name"
    );
    let uid = rustix::process::geteuid().as_raw();
    let metadata = fs::symlink_metadata(runtime).context("inspect runtime directory")?;
    ensure!(
        runtime.is_absolute()
            && runtime.canonicalize()? == runtime
            && metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && metadata.uid() == uid
            && metadata.mode() & 0o777 == 0o700,
        "runtime directory must be owned by this uid, canonical and mode 0700"
    );
    let lock: File = open(
        runtime.join(format!(".{name}.sock.lock")),
        OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::RUSR | Mode::WUSR,
    )?
    .into();
    let meta = lock.metadata()?;
    ensure!(
        meta.is_file() && meta.uid() == uid && meta.mode() & 0o777 == 0o600 && meta.nlink() == 1,
        "unsafe socket lock file"
    );
    flock(&lock, FlockOperation::NonBlockingLockExclusive)
        .context("socket listener already owned")?;
    let path = runtime.join(format!("{name}.sock"));
    match fs::symlink_metadata(&path) {
        Ok(meta) => {
            ensure!(
                meta.file_type().is_socket() && meta.uid() == uid,
                "refusing to replace non-socket or foreign socket"
            );
            match tokio::time::timeout(Duration::from_millis(200), UnixStream::connect(&path)).await
            {
                Ok(Err(error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => {}
                _ => anyhow::bail!("refusing to replace a live or unresponsive socket"),
            }
            let current = fs::symlink_metadata(&path)?;
            ensure!(
                (meta.dev(), meta.ino()) == (current.dev(), current.ino()),
                "socket changed during stale check"
            );
            fs::remove_file(&path)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let listener = UnixListener::bind(&path).context("bind JSONL socket")?;
    let meta = fs::symlink_metadata(&path)?;
    let owned = JsonlListener {
        listener,
        path,
        identity: (meta.dev(), meta.ino()),
        uid,
        _lock: lock,
    };
    fs::set_permissions(owned.path(), fs::Permissions::from_mode(0o700))?;
    Ok(owned)
}
