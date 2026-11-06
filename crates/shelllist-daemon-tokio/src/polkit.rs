//! Bounded polkit checks against the verified unique D-Bus sender.
use std::{collections::HashMap, time::Duration};
use zbus::zvariant::Value;

/// Authorize an action for a sender obtained from the incoming message header.
/// Callers own action IDs, interaction policy, time budgets and domain errors.
/// Expiry or owner loss cancels the polkit request and fails closed. No operation
/// should begin until this function succeeds. Dropping a dispatched operation
/// cannot imply rollback of an effect that has already started.
pub async fn authorize_polkit(
    connection: &zbus::Connection,
    sender: &zbus::names::UniqueName<'_>,
    action: &str,
    budget: Duration,
    interactive: bool,
) -> zbus::fdo::Result<()> {
    let proxy = zbus::Proxy::new(
        connection,
        "org.freedesktop.PolicyKit1",
        "/org/freedesktop/PolicyKit1/Authority",
        "org.freedesktop.PolicyKit1.Authority",
    )
    .await
    .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
    let subject_details = HashMap::from([("name", Value::from(sender.as_str()))]);
    let subject = ("system-bus-name", subject_details);
    let details = HashMap::<&str, &str>::new();
    // A separate cancellation ID per request; never supplied as caller identity.
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let cancellation_id = format!(
        "shelllist-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let monitor = crate::OwnerLossMonitor::new(connection.clone());
    let body = (
        subject,
        action,
        details,
        u32::from(interactive),
        cancellation_id.as_str(),
    );
    let check =
        proxy.call::<_, _, (bool, bool, HashMap<String, String>)>("CheckAuthorization", &body);
    let result = tokio::select! {
        result = tokio::time::timeout(budget, check) => result.ok(),
        _ = monitor.wait(sender.as_str()) => None,
    };
    let Some(result) = result else {
        // Dropping a local wait alone does not cancel polkit. Bound cleanup too.
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            proxy.call::<_, _, ()>("CancelCheckAuthorization", &(cancellation_id.as_str(),)),
        )
        .await;
        return Err(zbus::fdo::Error::AccessDenied(
            "authorization cancelled or expired; no write started".into(),
        ));
    };
    let (authorized, _challenge, _details) =
        result.map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?;
    // Owner loss may race a successful authorization. No await between this
    // check returning and the synchronous writer. A write already begun cannot
    // be rolled back by disconnect/cancellation.
    if !monitor
        .has_owner(sender.as_str())
        .await
        .map_err(|error| zbus::fdo::Error::Failed(error.to_string()))?
    {
        return Err(zbus::fdo::Error::AccessDenied(
            "caller disconnected before write".into(),
        ));
    }
    if authorized {
        Ok(())
    } else {
        Err(zbus::fdo::Error::AccessDenied(
            "action was not authorized".into(),
        ))
    }
}
