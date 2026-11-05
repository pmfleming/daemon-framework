use std::fmt;

/// Delivery classification survives `anyhow` context via `downcast_ref`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcError {
    NotDelivered,
    OutcomeUnknown,
    Rejected(String),
    InvalidReply,
    SessionGone { configured: String, live: String },
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotDelivered => f.write_str("Hyprland request was not delivered"),
            Self::OutcomeUnknown => f.write_str("Hyprland request outcome is unknown"),
            Self::Rejected(reply) => write!(f, "Hyprland rejected command: {reply}"),
            Self::InvalidReply => f.write_str("invalid Hyprland reply"),
            Self::SessionGone { configured, live } => write!(
                f,
                "Hyprland session {configured} is gone; live instance is {live}"
            ),
        }
    }
}
impl std::error::Error for IpcError {}

/// A single-command acknowledgement. JSON batches with an `ok` prefix are a
/// separate protocol and must parse their remaining JSON instead.
pub fn ack(reply: &str) -> anyhow::Result<()> {
    match reply.trim() {
        "ok" => Ok(()),
        "" => Err(IpcError::InvalidReply.into()),
        reply => Err(IpcError::Rejected(reply.to_owned()).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn acknowledgement_is_exact_and_errors_survive_context() {
        assert!(ack("  ok\n").is_ok());
        for reply in ["okay", "ok but failed", "error", "ok\nok"] {
            let error = ack(reply).unwrap_err().context("operation");
            assert_eq!(
                error.downcast_ref::<IpcError>(),
                Some(&IpcError::Rejected(reply.into()))
            );
        }
        assert_eq!(
            ack("").unwrap_err().downcast_ref::<IpcError>(),
            Some(&IpcError::InvalidReply)
        );
    }
}
