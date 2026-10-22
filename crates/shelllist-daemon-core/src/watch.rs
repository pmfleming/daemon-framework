//! Watchers are optional accelerators: recovery is rate-limited and polling stays
//! the caller's responsibility. Register returned paths nonrecursively so the
//! backend cannot grow an unbounded recursive watch set behind our budget.
use std::{
    collections::BTreeSet,
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

pub const DEFAULT_WATCH_LIMIT: usize = 512;

#[derive(Debug)]
pub struct WatchBackoff {
    retry_at: Instant,
    delay: Duration,
    healthy_since: Option<Instant>,
}

impl WatchBackoff {
    pub fn new(now: Instant) -> Self {
        Self {
            retry_at: now,
            delay: Duration::from_secs(1),
            healthy_since: None,
        }
    }

    pub fn deadline(&self) -> Instant {
        self.retry_at
    }
    pub fn ready(&self, now: Instant) -> bool {
        now >= self.retry_at
    }

    /// Construction alone is not recovery: repeated construct/fail cycles must
    /// retain their penalty until a watcher survives a full healthy interval.
    pub fn started(&mut self, now: Instant) {
        self.healthy_since = Some(now);
    }

    pub fn failed(&mut self, now: Instant) {
        if self
            .healthy_since
            .is_some_and(|start| now.saturating_duration_since(start) >= Duration::from_secs(60))
        {
            self.delay = Duration::from_secs(1);
        }
        self.healthy_since = None;
        self.retry_at = now + self.delay;
        self.delay = (self.delay * 2).min(Duration::from_secs(30));
    }
}

/// Explicit roots may be symlinks (e.g. a Nix profile); descendant symlinks are
/// never traversed. An incomplete plan is an error, not healthy partial coverage.
pub fn bounded_watch_paths(
    roots: impl IntoIterator<Item = (PathBuf, bool)>,
    limit: usize,
) -> io::Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();
    let mut expanded = BTreeSet::new();
    let mut pending = roots.into_iter();
    for (root, recursive) in &mut pending {
        let mut stack = vec![(root, recursive)];
        while let Some((path, recursive)) = stack.pop() {
            if paths.insert(path.clone()) && paths.len() > limit {
                return Err(io::Error::other(
                    "watch registration budget exceeded; use polling",
                ));
            }
            if recursive && expanded.insert(path.clone()) {
                for entry in std::fs::read_dir(path)? {
                    let entry = entry?;
                    if entry.file_type()?.is_dir() {
                        // Bound queued directories too, before traversing them.
                        if paths.len() + stack.len() >= limit {
                            return Err(io::Error::other(
                                "watch registration budget exceeded; use polling",
                            ));
                        }
                        stack.push((entry.path(), true));
                    }
                }
            }
        }
    }
    Ok(paths.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flapping_construction_and_failures_back_off_until_sustained_health() {
        let mut now = Instant::now();
        let mut backoff = WatchBackoff::new(now);
        for seconds in [1, 2, 4, 8, 16, 30, 30] {
            assert!(backoff.ready(now));
            backoff.started(now);
            backoff.failed(now);
            assert_eq!(backoff.deadline() - now, Duration::from_secs(seconds));
            assert!(!backoff.ready(now));
            now = backoff.deadline();
        }
        backoff.started(now);
        now += Duration::from_secs(60);
        backoff.failed(now);
        assert_eq!(backoff.deadline() - now, Duration::from_secs(1));
    }

    #[test]
    fn watch_plan_is_bounded_and_does_not_follow_descendant_symlinks() {
        let dir = crate::file::tests::Directory::new();
        let root = dir.0.join("root");
        std::fs::create_dir_all(root.join("child/grandchild")).unwrap();
        std::fs::create_dir_all(dir.0.join("external/closure")).unwrap();
        std::os::unix::fs::symlink(dir.0.join("external"), root.join("link")).unwrap();
        assert_eq!(
            bounded_watch_paths([(root.clone(), true)], 3)
                .unwrap()
                .len(),
            3
        );
        assert!(bounded_watch_paths([(root.clone(), true)], 2).is_err());
        assert_eq!(
            bounded_watch_paths([(root.clone(), false)], 1).unwrap(),
            std::slice::from_ref(&root)
        );
        assert!(bounded_watch_paths([(root, false)], 0).is_err());
    }
}
