use std::sync::atomic::{AtomicU64, Ordering};

/// Process-local monotonic identifier source.
#[derive(Debug)]
pub struct IdSequence {
    next: AtomicU64,
}

impl IdSequence {
    #[must_use]
    pub const fn new(first: u64) -> Self {
        Self {
            next: AtomicU64::new(first),
        }
    }

    #[must_use]
    pub fn next(&self, prefix: &str) -> String {
        format!("{prefix}-{}", self.next.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for IdSequence {
    fn default() -> Self {
        Self::new(1)
    }
}

#[cfg(test)]
mod tests {
    use super::IdSequence;

    #[test]
    fn generates_prefixed_monotonic_ids() {
        let ids = IdSequence::default();
        assert_eq!(ids.next("subscription"), "subscription-1");
        assert_eq!(ids.next("request"), "request-2");
    }
}
