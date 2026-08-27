//! Runtime-independent infrastructure shared by Shelllist daemons.

mod endpoint;
mod envelope;
mod event;
mod id;
mod jsonl_wire;
mod protocol;

pub use endpoint::{ApiIdentity, DaemonEndpoint};
pub use envelope::{ApiError, error, success};
pub use event::{Correlation, event_envelope};
pub use id::IdSequence;
pub use jsonl_wire::{
    ClientRequest, event_message, protocol_error_message, response_error_message, response_message,
    shutdown_message, transport_error_message,
};
pub use protocol::{fixture_names, load_fixture, validate_unique_names};
