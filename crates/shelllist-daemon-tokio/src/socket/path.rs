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
    lock: File,
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
        // Closing our descriptor alone does not release flock if a concurrent
        // fork inherited the same open file description and has not exec'd yet.
        // Release ownership after path cleanup, without unlinking the lock inode.
        let _ = flock(&self.lock, FlockOperation::Unlock);
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
        lock,
    };
    fs::set_permissions(owned.path(), fs::Permissions::from_mode(0o700))?;
    Ok(owned)
}

#[cfg(test)]
#[tokio::test]
async fn shutdown_releases_lock_even_with_an_inherited_descriptor() {
    use std::sync::atomic::Ordering;

    let directory = std::env::temp_dir().join(format!(
        "jsonl-lock-{}-{}",
        std::process::id(),
        super::NEXT_OWNER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&directory).unwrap();
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
    let listener = bind(&directory, "test").await.unwrap();
    // dup shares the open file description just like fork before exec closes
    // CLOEXEC descriptors. Avoid a timing-dependent concurrent process spawn.
    let inherited = listener.lock.try_clone().unwrap();
    assert!(bind(&directory, "test").await.is_err());
    drop(listener);
    let restarted = bind(&directory, "test").await;
    drop(inherited);
    let restarted =
        restarted.expect("shutdown must release ownership before exec in another process");
    assert!(bind(&directory, "test").await.is_err());
    drop(restarted);
    fs::remove_dir_all(directory).unwrap();
}
