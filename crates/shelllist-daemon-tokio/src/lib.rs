//! Tokio and session D-Bus infrastructure shared by Shelllist daemons.

mod dbus;
mod output_actor;
mod shutdown;
mod subscription;

pub use dbus::{JsonDbusClient, directed_emitter, wait_for_owner_loss, watch_name_replacement};
pub use output_actor::{
    BasicCorrelation, CorrelationPolicy, OutputCommand, OutputHandle, TrackedId, TrackedKind,
    spawn_output_actor, spawn_output_actor_with_writer,
};
pub use shutdown::wait_for_shutdown;
pub use subscription::OwnedTaskRegistry;
