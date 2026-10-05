use std::{
    future::Future,
    sync::{Arc, Mutex, OnceLock, Weak},
};

use shelllist_daemon_core::{
    IdSequence, OperationAdmissionError, OperationLimits, OwnedOperations,
};
use tokio::{sync::oneshot, task::JoinHandle};

use crate::{
    owner::OwnerLossMonitor,
    task::{abort_and_join, spawn_named},
};

struct OwnedTask {
    task: JoinHandle<()>,
    generation: Arc<()>,
}

struct State {
    closed: bool,
    tasks: OwnedOperations<OwnedTask>,
}

/// One registry belongs to one serving D-Bus connection. No global bus state.
pub struct OwnedTaskRegistry {
    ids: IdSequence,
    state: Arc<Mutex<State>>,
    owners: OnceLock<OwnerLossMonitor>,
}

#[derive(Debug, Clone, Copy)]
pub struct TaskLimits {
    pub total: usize,
    pub per_owner: usize,
}

impl Default for TaskLimits {
    fn default() -> Self {
        Self {
            total: 256,
            per_owner: 64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAdmissionError {
    Closed,
    Full,
    DuplicateId,
}
impl std::fmt::Display for TaskAdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Closed => "task registry is shutting down",
            Self::Full => "too many active tasks; retry after a task finishes",
            Self::DuplicateId => "task ID is already active",
        })
    }
}
impl std::error::Error for TaskAdmissionError {}

impl From<OperationAdmissionError> for TaskAdmissionError {
    fn from(error: OperationAdmissionError) -> Self {
        match error {
            OperationAdmissionError::Full => Self::Full,
            OperationAdmissionError::DuplicateId => Self::DuplicateId,
        }
    }
}

struct Registration {
    state: Weak<Mutex<State>>,
    id: String,
    generation: Arc<()>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            let mut state = state.lock().unwrap_or_else(|p| p.into_inner());
            if state
                .tasks
                .get_mut(&self.id)
                .is_some_and(|task| Arc::ptr_eq(&task.generation, &self.generation))
            {
                state.tasks.claim(&self.id);
            }
        }
    }
}

impl OwnedTaskRegistry {
    #[must_use]
    pub fn new(first_id: u64) -> Self {
        Self::with_limits(first_id, TaskLimits::default())
    }

    #[must_use]
    pub fn with_limits(first_id: u64, limits: TaskLimits) -> Self {
        Self {
            ids: IdSequence::new(first_id),
            state: Arc::new(Mutex::new(State {
                closed: false,
                tasks: OwnedOperations::new(OperationLimits {
                    total: limits.total,
                    per_owner: limits.per_owner,
                }),
            })),
            owners: OnceLock::new(),
        }
    }

    #[must_use]
    pub fn next_id(&self, prefix: &str) -> String {
        self.ids.next(prefix)
    }

    /// Lazily binds this registry's monitor to its serving connection.
    pub fn owner_monitor(&self, connection: &zbus::Connection) -> OwnerLossMonitor {
        self.owners
            .get_or_init(|| OwnerLossMonitor::new(connection.clone()))
            .clone()
    }

    /// Registers before polling the worker. Cleans up on completion, panic,
    /// cancellation, owner loss, monitor failure, or registry destruction.
    pub fn spawn_for_owner(
        &self,
        id: String,
        owner: Option<String>,
        connection: &zbus::Connection,
        events: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), TaskAdmissionError> {
        let monitor = self.owner_monitor(connection);
        let watched_owner = owner.clone();
        self.spawn_until(id, owner, async move {
            match watched_owner {
                Some(owner) => {
                    if let Err(error) = monitor.wait(&owner).await {
                        tracing::warn!(%owner, %error, "ending owned task because owner monitoring failed");
                    }
                }
                None => std::future::pending().await,
            }
        }, events)
    }

    pub fn spawn_until(
        &self,
        id: String,
        owner: Option<String>,
        stop: impl Future<Output = ()> + Send + 'static,
        events: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), TaskAdmissionError> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.closed {
            return Err(TaskAdmissionError::Closed);
        }
        let (start, ready) = oneshot::channel();
        state.tasks.insert_with(id, owner, |id| {
            let generation = Arc::new(());
            let registration = Registration {
                state: Arc::downgrade(&self.state),
                id: id.to_owned(),
                generation: generation.clone(),
            };
            let task = spawn_named("owned-task", async move {
                let _registration = registration;
                if ready.await.is_err() {
                    return;
                }
                tokio::select! { biased; () = stop => {}, () = events => {} }
            });
            OwnedTask { task, generation }
        })?;
        drop(state);
        let _ = start.send(());
        Ok(())
    }

    pub async fn cancel_owned(&self, id: &str, owner: Option<&str>) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.tasks.claim_owned(id, owner).is_some_and(|task| {
            task.value.task.abort();
            true
        })
    }

    /// Closes admission before aborting and joining every registered worker.
    pub async fn shutdown(&self) {
        let tasks = {
            let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
            state.closed = true;
            state
                .tasks
                .drain()
                .map(|task| task.value.task)
                .collect::<Vec<_>>()
        };
        abort_and_join(tasks).await;
    }
}

impl Default for OwnedTaskRegistry {
    fn default() -> Self {
        Self::new(1)
    }
}
impl Drop for OwnedTaskRegistry {
    fn drop(&mut self) {
        for (_, task) in self
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .tasks
            .iter()
        {
            task.value.task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{OwnedTaskRegistry, TaskAdmissionError, TaskLimits};
    use std::{future::pending, sync::Arc};

    #[tokio::test]
    async fn admission_cancellation_and_shutdown_are_owner_scoped() {
        let registry = OwnedTaskRegistry::with_limits(
            1,
            TaskLimits {
                total: 2,
                per_owner: 1,
            },
        );
        registry
            .spawn_until("one".into(), Some("a".into()), pending(), pending())
            .unwrap();
        for (id, error) in [
            ("one", TaskAdmissionError::DuplicateId),
            ("two", TaskAdmissionError::Full),
        ] {
            assert_eq!(
                registry.spawn_until(id.into(), Some("a".into()), pending(), pending()),
                Err(error)
            );
        }
        registry
            .spawn_until("two".into(), Some("b".into()), pending(), pending())
            .unwrap();
        assert_eq!(
            registry.spawn_until("three".into(), Some("c".into()), pending(), pending()),
            Err(TaskAdmissionError::Full)
        );
        assert!(!registry.cancel_owned("one", Some("b")).await);
        assert!(registry.cancel_owned("one", Some("a")).await);
        assert!(registry.cancel_owned("two", Some("b")).await);
        registry.shutdown().await;
        assert_eq!(
            registry.spawn_until("late".into(), None, pending(), pending()),
            Err(TaskAdmissionError::Closed)
        );
    }

    #[tokio::test]
    async fn immediate_completion_panic_and_stop_remove_registration() {
        let registry = OwnedTaskRegistry::default();
        registry
            .spawn_until("complete".into(), None, pending(), async {})
            .unwrap();
        registry
            .spawn_until("panic".into(), None, pending(), async {
                panic!("injected")
            })
            .unwrap();
        registry
            .spawn_until("stop".into(), None, async {}, pending())
            .unwrap();
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert_eq!(registry.state.lock().unwrap().tasks.iter().count(), 0);
    }

    #[tokio::test]
    async fn old_cleanup_cannot_remove_reused_id() {
        let registry = OwnedTaskRegistry::default();
        registry
            .spawn_until("id".into(), None, pending(), pending())
            .unwrap();
        assert!(registry.cancel_owned("id", None).await);
        registry
            .spawn_until("id".into(), None, pending(), pending())
            .unwrap();
        tokio::task::yield_now().await;
        assert!(registry.cancel_owned("id", None).await);
    }

    #[tokio::test]
    async fn dropping_registry_releases_workers() {
        let registry = OwnedTaskRegistry::default();
        let marker = Arc::new(());
        let held = marker.clone();
        registry
            .spawn_until("id".into(), None, pending(), async move {
                let _held = held;
                pending::<()>().await;
            })
            .unwrap();
        drop(registry);
        tokio::task::yield_now().await;
        assert_eq!(Arc::strong_count(&marker), 1);
    }
}
