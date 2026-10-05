use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::{self, File, FileTimes},
    io,
    os::unix::{ffi::OsStrExt, fs::symlink},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};

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
    let output = Command::new("git")
        .arg("-c")
        .arg(format!("safe.directory={}", root.display()))
        .arg("-C")
        .arg(root)
        .args(args)
        .stderr(Stdio::inherit())
        .output()
        .context("run git")?;
    ensure!(
        output.status.success(),
        "git {args:?} failed: {}",
        output.status
    );
    Ok(output.stdout)
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
        if metadata.is_symlink() {
            ensure!(
                resolve(&source)?.starts_with(root),
                "symlink escapes snapshot: {}",
                source.display()
            );
            symlink(fs::read_link(&source)?, &target)?;
        } else if metadata.is_file() {
            copy_file(&source, &target)?;
            let after = fs::metadata(&source)?;
            ensure!(
                (metadata.modified()?, metadata.len()) == (after.modified()?, after.len()),
                "file changed while snapshotting; retry: {}",
                source.display()
            );
        } else {
            bail!(
                "unsupported tracked directory/submodule: {}",
                source.display()
            );
        }
    }
    Ok(())
}

fn copy_file(source: &Path, target: &Path) -> Result<()> {
    fs::copy(source, target).with_context(|| format!("copy {}", source.display()))?;
    let metadata = fs::metadata(source)?;
    File::open(target)?.set_times(
        FileTimes::new()
            .set_accessed(metadata.accessed()?)
            .set_modified(metadata.modified()?),
    )?;
    Ok(())
}

/// Copy an already frozen tree without dereferencing or rewriting symlinks.
pub(crate) fn copy_tree(source: &Path, target: &Path) -> Result<()> {
    fs::create_dir(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            symlink(fs::read_link(entry.path())?, destination)?;
        } else if kind.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else if kind.is_file() {
            copy_file(&entry.path(), &destination)?;
        } else {
            bail!("unsupported snapshot entry: {}", entry.path().display());
        }
    }
    fs::set_permissions(target, fs::metadata(source)?.permissions())?;
    Ok(())
}
