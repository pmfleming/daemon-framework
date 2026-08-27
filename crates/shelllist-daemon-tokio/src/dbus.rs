use anyhow::{Context, Result};
use futures::StreamExt;
use serde_json::{Value, json};
use zbus::{message::Header, names::UniqueName, object_server::SignalEmitter};

use shelllist_daemon_core::DaemonEndpoint;

#[derive(Clone)]
pub struct JsonDbusClient {
    connection: zbus::Connection,
    endpoint: DaemonEndpoint,
}

impl JsonDbusClient {
    #[must_use]
    pub const fn new(connection: zbus::Connection, endpoint: DaemonEndpoint) -> Self {
        Self {
            connection,
            endpoint,
        }
    }

    pub async fn session(endpoint: DaemonEndpoint) -> Result<Self> {
        let connection = zbus::Connection::session()
            .await
            .context("connect to session D-Bus")?;
        Ok(Self::new(connection, endpoint))
    }

    pub fn connection(&self) -> &zbus::Connection {
        &self.connection
    }

    async fn proxy(&self) -> Result<zbus::Proxy<'_>> {
        zbus::Proxy::new(
            &self.connection,
            self.endpoint.bus_name,
            self.endpoint.object_path,
            self.endpoint.interface,
        )
        .await
        .with_context(|| format!("create {} proxy", self.endpoint.executable))
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let proxy = self.proxy().await?;
        let params_json = serde_json::to_string(&params).context("encode call parameters")?;
        let response: String = proxy
            .call("Call", &(method, params_json.as_str()))
            .await
            .with_context(|| format!("call {}", self.endpoint.executable))?;
        serde_json::from_str(&response).context("decode daemon call response")
    }

    pub async fn subscribe(&self, streams: Vec<String>) -> Result<Value> {
        let proxy = self.proxy().await?;
        let response: String = proxy
            .call("Subscribe", &(streams,))
            .await
            .with_context(|| format!("subscribe to {}", self.endpoint.executable))?;
        serde_json::from_str(&response).context("decode daemon subscription response")
    }

    pub async fn cancel_json(&self, request_id: &str) -> Result<Value> {
        let proxy = self.proxy().await?;
        let response: String = proxy
            .call("Cancel", &(request_id,))
            .await
            .with_context(|| format!("cancel {} request", self.endpoint.executable))?;
        serde_json::from_str(&response).context("decode daemon cancellation response")
    }

    pub async fn cancel_unit(&self, request_id: &str) -> Result<Value> {
        let proxy = self.proxy().await?;
        proxy
            .call::<_, _, ()>("Cancel", &(request_id,))
            .await
            .with_context(|| format!("cancel {} request", self.endpoint.executable))?;
        Ok(json!({ "cancelled": request_id }))
    }
}

#[must_use]
pub fn directed_emitter(
    emitter: &SignalEmitter<'_>,
    header: &Header<'_>,
) -> SignalEmitter<'static> {
    match header.sender() {
        Some(sender) => emitter.to_owned().set_destination(sender.to_owned().into()),
        None => emitter.to_owned(),
    }
}

pub async fn wait_for_owner_loss(
    connection: &zbus::Connection,
    owner: UniqueName<'static>,
) -> Result<()> {
    let proxy = zbus::Proxy::new(
        connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await
    .context("create D-Bus owner proxy")?;
    let mut changes = proxy
        .receive_signal("NameOwnerChanged")
        .await
        .context("receive D-Bus owner changes")?;
    let has_owner: bool = proxy
        .call("NameHasOwner", &(owner.as_str(),))
        .await
        .context("check D-Bus owner")?;
    if !has_owner {
        return Ok(());
    }
    while let Some(message) = changes.next().await {
        let (name, old_owner, new_owner): (String, String, String) =
            message
                .body()
                .deserialize()
                .context("decode owner change")?;
        if name == owner.as_str() && !old_owner.is_empty() && new_owner.is_empty() {
            return Ok(());
        }
    }
    anyhow::bail!("D-Bus owner-change stream ended")
}

pub async fn watch_name_replacement(connection: &zbus::Connection, bus_name: &str) -> Result<()> {
    let proxy = zbus::Proxy::new(
        connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await
    .context("create D-Bus owner proxy")?;
    let mut changes = proxy
        .receive_signal("NameOwnerChanged")
        .await
        .context("receive D-Bus owner changes")?;
    while let Some(message) = changes.next().await {
        let (name, old_owner, new_owner): (String, String, String) =
            message
                .body()
                .deserialize()
                .context("decode owner change")?;
        if name == bus_name && !old_owner.is_empty() && old_owner != new_owner {
            return Ok(());
        }
    }
    anyhow::bail!("D-Bus owner-change stream ended")
}
