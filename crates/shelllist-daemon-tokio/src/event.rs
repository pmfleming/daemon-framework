//! Transport-neutral event destination. Ownership/routing is selected at ingress,
//! never inferred from event payloads or caller-supplied JSON.
use crate::SocketSession;
use anyhow::Result;
use serde_json::Value;
use zbus::object_server::SignalEmitter;

#[derive(Clone)]
pub enum EventEmitter<'a> {
    Dbus(SignalEmitter<'a>),
    Socket(SocketSession),
}

impl<'a> From<SignalEmitter<'a>> for EventEmitter<'a> {
    fn from(emitter: SignalEmitter<'a>) -> Self {
        Self::Dbus(emitter)
    }
}
impl From<SocketSession> for EventEmitter<'static> {
    fn from(session: SocketSession) -> Self {
        Self::Socket(session)
    }
}
impl EventEmitter<'_> {
    pub fn into_owned(self) -> EventEmitter<'static> {
        match self {
            Self::Dbus(emitter) => EventEmitter::Dbus(emitter.into_owned()),
            Self::Socket(session) => EventEmitter::Socket(session),
        }
    }
    /// D-Bus lifetime is still monitored by its owner's bus-name watcher.
    pub fn is_closed(&self) -> bool {
        match self {
            Self::Dbus(_) => false,
            Self::Socket(session) => session.is_closed(),
        }
    }
    pub async fn emit(&self, interface: &str, stream: &str, event: Value) -> Result<()> {
        match self {
            Self::Dbus(emitter) => Ok(emitter
                .emit(interface, "Event", &(stream, event.to_string()))
                .await?),
            Self::Socket(session) => session.emit(stream, event).await,
        }
    }
    /// For existing synchronous producers: D-Bus retains its blocking emission;
    /// sockets never block a runtime worker on the output queue. Saturation
    /// retires that connection, so a lost event cannot silently poison a delta
    /// baseline and one slow consumer cannot stall the shared producer actor.
    pub fn emit_sync(&self, interface: &str, stream: &str, event: Value) -> Result<()> {
        match self {
            Self::Dbus(_) => futures::executor::block_on(self.emit(interface, stream, event)),
            Self::Socket(session) => session.try_emit(stream, event),
        }
    }
}
