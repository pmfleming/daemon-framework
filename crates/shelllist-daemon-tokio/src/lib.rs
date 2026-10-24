//! Tokio and session D-Bus infrastructure shared by Shelllist daemons.

#[cfg(test)]
mod transport_tests;

mod blocking;
mod dbus;
pub use blocking::{BlockingLane, LaneError, LaneShutdown};
mod event;
pub use event::EventEmitter;
mod file;
mod forward;
mod jsonl;
mod socket;
pub use file::{read_bytes_bounded_async, write_bytes_atomic_async};
pub use forward::{BroadcastEvent, WatchPhase, emit_event_body, forward_broadcast, forward_watch};
pub use socket::{JsonlListener, JsonlSocketConfig, LocalJsonlService, SocketSession};
mod output_actor;
mod owner;
pub use owner::{OwnerLossError, OwnerLossMonitor, OwnerLossReceiver};
mod resume;
mod service;
mod shutdown;
pub use service::{Admitted, ServiceBoundary};
mod task;

pub use resume::{ResumeDetector, monitor_resumes, suspend_offset};
pub use task::{AbortOnDrop, TaskGroup, catch_task, spawn_named};
mod subscription;

pub use dbus::{JsonDbusClient, directed_emitter};
pub use jsonl::{JsonlClientConfig, run_jsonl_client, run_jsonl_client_with_input_limit};
pub use output_actor::{BasicCorrelation, CorrelationPolicy, TrackedId, TrackedKind};
pub use shutdown::wait_for_shutdown;
pub use subscription::{OwnedTaskRegistry, TaskAdmissionError, TaskLimits};
