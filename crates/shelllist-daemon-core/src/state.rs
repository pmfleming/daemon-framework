use serde::{Serialize, de::DeserializeOwned};
use std::env;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::file::{AtomicFilePolicy, StagedFile};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdgRoot {
    Config,
    State,
    Cache,
    Runtime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtomicWritePolicy {
    pub directory_mode: u32,
    pub file_mode: u32,
    pub pretty: bool,
    pub sync_parent: bool,
}

impl AtomicWritePolicy {
    pub const PRIVATE: Self = Self {
        directory_mode: 0o700,
        file_mode: 0o600,
        pretty: true,
        sync_parent: true,
    };
}

#[derive(Debug)]
pub enum StateError {
    Io(io::Error),
    Json(serde_json::Error),
    InvalidPath,
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "state I/O failed: {error}"),
            Self::Json(error) => write!(formatter, "state JSON failed: {error}"),
            Self::InvalidPath => formatter.write_str("state path has no parent or file name"),
        }
    }
}

impl std::error::Error for StateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::InvalidPath => None,
        }
    }
}

impl From<io::Error> for StateError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for StateError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[must_use]
pub fn resolve_xdg_root(root: XdgRoot) -> Option<PathBuf> {
    match root {
        XdgRoot::Config => xdg_home("XDG_CONFIG_HOME", ".config"),
        XdgRoot::State => xdg_home("XDG_STATE_HOME", ".local/state"),
        XdgRoot::Cache => xdg_home("XDG_CACHE_HOME", ".cache"),
        XdgRoot::Runtime => absolute_env("XDG_RUNTIME_DIR"),
    }
}

fn xdg_home(variable: &str, fallback: &str) -> Option<PathBuf> {
    absolute_env(variable).or_else(|| absolute_env("HOME").map(|home| home.join(fallback)))
}

fn absolute_env(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

/// Resolves a path below one application's XDG directory.
///
/// The application must be one non-empty path component, and every component
/// of `path` must be a normal relative component. Absolute paths, parent
/// traversal, and empty paths are rejected.
#[must_use]
pub fn resolve_xdg_path(root: XdgRoot, application: &str, path: &Path) -> Option<PathBuf> {
    let application = Path::new(application);
    if !is_single_normal_component(application) || !is_safe_relative_path(path) {
        return None;
    }
    resolve_xdg_root(root).map(|base| base.join(application).join(path))
}

fn is_single_normal_component(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StateError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn write_json_atomic<T: Serialize + ?Sized>(
    path: &Path,
    value: &T,
    policy: AtomicWritePolicy,
) -> Result<(), StateError> {
    StagedFile::with_writer::<StateError>(
        path,
        AtomicFilePolicy {
            directory_mode: Some(policy.directory_mode),
            file_mode: Some(policy.file_mode),
            sync_parent: policy.sync_parent,
            sync_new_ancestors: policy.sync_parent,
            ..AtomicFilePolicy::PRIVATE
        },
        |output| {
            if policy.pretty {
                serde_json::to_writer_pretty(output, value)?;
            } else {
                serde_json::to_writer(output, value)?;
            }
            Ok(())
        },
    )?
    .commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_safe_relative_path, is_single_normal_component};
    use std::path::Path;

    #[test]
    fn streamed_json_matches_encoding_and_serializer_failure_preserves_state() {
        use super::{AtomicWritePolicy, StateError, write_json_atomic};
        use serde::Serialize;
        use std::fs;
        let root = crate::file::tests::Directory::new();
        let path = root.0.join("state.json");
        let value = serde_json::json!({"large": "x".repeat(32_000), "unicode": "λ"});
        for pretty in [false, true] {
            write_json_atomic(
                &path,
                &value,
                AtomicWritePolicy {
                    pretty,
                    ..AtomicWritePolicy::PRIVATE
                },
            )
            .unwrap();
            let expected = if pretty {
                serde_json::to_vec_pretty(&value)
            } else {
                serde_json::to_vec(&value)
            }
            .unwrap();
            assert_eq!(fs::read(&path).unwrap(), expected);
        }
        struct Failed;
        impl Serialize for Failed {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                use serde::ser::SerializeSeq;
                let mut sequence = serializer.serialize_seq(Some(2))?;
                sequence.serialize_element(&"x".repeat(32_000))?;
                Err(serde::ser::Error::custom("injected failure"))
            }
        }
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            write_json_atomic(&path, &Failed, AtomicWritePolicy::PRIVATE),
            Err(StateError::Json(_))
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
    }

    #[test]
    fn xdg_suffixes_cannot_escape_the_application_directory() {
        assert!(is_single_normal_component(Path::new("shelllist")));
        assert!(is_safe_relative_path(Path::new("daemon/state.json")));

        for application in ["", ".", "..", "shelllist/daemon", "/shelllist"] {
            assert!(!is_single_normal_component(Path::new(application)));
        }
        for path in [
            "",
            ".",
            "../state.json",
            "daemon/../../state.json",
            "/tmp/state.json",
        ] {
            assert!(!is_safe_relative_path(Path::new(path)));
        }
    }
}
