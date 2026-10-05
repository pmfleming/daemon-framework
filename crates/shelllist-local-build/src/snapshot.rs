use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::{self, File, FileTimes, Metadata},
    io,
    os::unix::{ffi::OsStrExt, fs::symlink},
    path::{Component, Path, PathBuf},
    process::Command,
};

use crate::command::output;
use anyhow::{Context, Result, ensure};

/// Like canonicalize, but permits missing suffixes (including dangling links).
pub(crate) fn resolve(path: &Path) -> Result<PathBuf> {
    resolve_links(&std::env::current_dir()?.join(path), 0)
}

fn resolve_links(path: &Path, depth: usize) -> Result<PathBuf> {
    ensure!(depth < 40, "too many symlinks resolving {}", path.display());
    let mut resolved = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                resolved.pop();
            }
            Component::CurDir => {}
            _ => {
                resolved.push(component);
                match fs::read_link(&resolved) {
                    Ok(target) => {
                        resolved.pop();
                        resolved = resolve_links(&resolved.join(target), depth + 1)?;
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
                        ) => {}
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("resolve {}", resolved.display()));
                    }
                }
            }
        }
    }
    Ok(resolved)
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    output(
        Command::new("git")
            .arg("-c")
            .arg(format!("safe.directory={}", root.display()))
            .arg("-C")
            .arg(root)
            .args(args),
    )
}

pub(crate) fn snapshot(root: &Path, destination: &Path) -> Result<()> {
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    let top = top.strip_suffix(b"\n").unwrap_or(&top);
    ensure!(
        resolve(Path::new(OsStr::from_bytes(top)))? == root,
        "not a repository root: {}",
        root.display()
    );
    let untracked = git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    ensure!(
        untracked.is_empty(),
        "Git-add or ignore untracked files in {}:\n{}",
        root.display(),
        String::from_utf8_lossy(&untracked).replace('\0', "\n")
    );
    fs::create_dir_all(destination)?;
    let files = git(root, &["ls-files", "--cached", "-z"])?;
    for entry in files
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .collect::<BTreeSet<_>>()
    {
        let relative = Path::new(OsStr::from_bytes(entry));
        let source = root.join(relative);
        let metadata = match fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).context("inspect tracked file"),
        };
        let target = destination.join(relative);
        fs::create_dir_all(target.parent().context("tracked file has no parent")?)?;
        ensure!(
            !metadata.is_symlink() || resolve(&source)?.starts_with(root),
            "symlink escapes snapshot: {}",
            source.display()
        );
        copy_leaf(&source, &target, &metadata)?;
    }
    Ok(())
}

// Both tracked and already-frozen sources preserve leaf metadata and reject
// special files. Only the tracked-source caller applies root confinement.
fn copy_leaf(source: &Path, target: &Path, before: &Metadata) -> Result<()> {
    if before.is_symlink() {
        return symlink(fs::read_link(source)?, target).context("copy symlink");
    }
    ensure!(
        before.is_file(),
        "unsupported snapshot entry: {}",
        source.display()
    );
    fs::copy(source, target).with_context(|| format!("copy {}", source.display()))?;
    let after = fs::metadata(source)?;
    ensure!(
        (before.modified()?, before.len()) == (after.modified()?, after.len()),
        "file changed while snapshotting; retry: {}",
        source.display()
    );
    File::open(target)?.set_times(
        FileTimes::new()
            .set_accessed(after.accessed()?)
            .set_modified(after.modified()?),
    )?;
    Ok(())
}

/// Copy an already frozen tree without dereferencing or rewriting symlinks.
pub(crate) fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        let metadata = entry.metadata()?; // DirEntry does not follow symlinks.
        if metadata.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else {
            copy_leaf(&entry.path(), &destination, &metadata)?;
        }
    }
    fs::set_permissions(target, fs::metadata(source)?.permissions())?;
    Ok(())
}
