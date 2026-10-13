//! Executable wire contracts. Rust DTOs describe structure; policy stays explicit.
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// One coordinated product cutover, not a negotiated range of supported versions.
pub const CONTRACT_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionMode {
    Read,
    Effect,
    Operation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Sensitivity {
    Public,
    SensitiveRead,
    Effect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum StreamSemantics {
    Snapshot,
    Invalidation,
    Transitions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MethodPolicy {
    pub execution: ExecutionMode,
    pub sensitivity: Sensitivity,
    pub conflict_domain: &'static str,
    pub ack_budget_ms: u32,
    pub request_bytes: usize,
    pub result_bytes: usize,
}

impl MethodPolicy {
    #[must_use]
    pub const fn new(
        execution: ExecutionMode,
        sensitivity: Sensitivity,
        conflict_domain: &'static str,
    ) -> Self {
        Self {
            execution,
            sensitivity,
            conflict_domain,
            ack_budget_ms: 30_000,
            request_bytes: 1024 * 1024,
            result_bytes: 16 * 1024 * 1024,
        }
    }
}

/// Input and output schemas differ deliberately: Serde defaults permit omitted
/// request fields, but fields always emitted by a reply remain required.
#[must_use]
pub fn method_schemas<P: JsonSchema, R: JsonSchema>() -> Value {
    json!({
        "params_schema": SchemaSettings::draft2020_12().for_deserialize().into_generator().into_root_schema_for::<P>(),
        "result_schema": SchemaSettings::draft2020_12().for_serialize().into_generator().into_root_schema_for::<R>(),
    })
}

/// One declaration links wire name, live parameter/result types, policy, handler,
/// registry and fixture reply constructor. Domain errors and validation stay local.
#[macro_export]
macro_rules! declare_methods {
    (
        api: $api:ty, error: $error:ty, reply: $reply:ty,
        decode: $decode:path, unsupported: $unsupported:expr;
        $($variant:ident {
            name: $name:literal,
            params: $params:ty,
            result: $result:ty,
            handler: $handler:ident,
            reply: $build:expr,
            policy: $policy:expr,
            stream: $stream:expr,
            example: $example:expr,
            $(pagination: $pagination:expr,)?
        })+
    ) => {
        pub const METHODS: &[&str] = &[$($name),+];
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Method { $($variant),+ }
        impl Method {
            pub const ALL: &[Self] = &[$(Self::$variant),+];
            #[deny(unreachable_patterns)]
            pub fn parse(name: &str) -> Option<Self> {
                match name { $($name => Some(Self::$variant),)+ _ => None }
            }
            pub const fn name(self) -> &'static str {
                match self { $(Self::$variant => $name),+ }
            }
            pub const fn policy(self) -> $crate::MethodPolicy {
                match self { $(Self::$variant => $policy),+ }
            }
            pub fn registry_entry(self) -> serde_json::Value {
                match self { $(Self::$variant => {
                    let params: $params = $example;
                    let mut entry = $crate::method_schemas::<$params, $result>();
                    entry["name"] = serde_json::json!($name);
                    entry["params_example"] = serde_json::json!(params);
                    entry["stream"] = serde_json::json!($stream);
                    entry["policy"] = serde_json::json!(self.policy());
                    $(entry["pagination"] = $pagination;)?
                    entry
                }),+ }
            }
        }
        pub(crate) enum Request { $($variant($params)),+ }
        impl Request {
            pub(crate) fn decode(name: &str, params: serde_json::Value) -> Result<Self, $error> {
                let method = Method::parse(name).ok_or_else(|| ($unsupported)(name))?;
                match method { $(Method::$variant => $decode(params).map(Self::$variant)),+ }
            }
            pub(crate) async fn dispatch(self, api: &$api, owner: Option<String>) -> Result<$reply, $error> {
                match self { $(Self::$variant(params) => {
                    let result: Result<$result, $error> = api.$handler(params, owner).await;
                    result.map($variant::reply)
                }),+ }
            }
        }
        $(pub(crate) struct $variant;
        impl $variant {
            pub(crate) fn reply(value: $result) -> $reply { ($build)(value) }
        })+
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default, Serialize, Deserialize, JsonSchema)]
    #[serde(deny_unknown_fields)]
    pub(crate) struct Params {
        #[serde(default)]
        count: u32,
    }
    pub(crate) struct Api;
    impl Api {
        async fn count(&self, params: Params, _owner: Option<String>) -> Result<u32, String> {
            Ok(params.count)
        }
    }
    fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, String> {
        serde_json::from_value(value).map_err(|error| error.to_string())
    }
    declare_methods! {
        api: Api, error: String, reply: Value,
        decode: decode, unsupported: |name| format!("Unsupported method: {name}");
        Count {
            name: "test.count",
            params: Params,
            result: u32,
            handler: count,
            reply: |count| json!({"count": count}),
            policy: MethodPolicy::new(ExecutionMode::Read, Sensitivity::Public, "test"),
            stream: "test.changed",
            example: Params { count: 1 },
        }
    }
    #[test]
    fn declaration_drives_decode_handler_reply_and_registry() {
        use std::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        assert_eq!(METHODS, &["test.count"]);
        assert_eq!(Method::ALL[0].name(), METHODS[0]);
        let entry = Method::Count.registry_entry();
        assert_eq!(entry["params_schema"]["additionalProperties"], false);
        assert_eq!(entry["result_schema"]["type"], "integer");
        assert!(Request::decode("missing", json!({})).is_err());
        assert!(Request::decode("test.count", json!({"count":"wrong"})).is_err());
        let request = Request::decode("test.count", json!({"count":4})).unwrap();
        let mut future = Box::pin(request.dispatch(&Api, None));
        assert_eq!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(json!({"count":4})))
        );
    }
    #[test]
    fn schemas_preserve_request_defaults_but_require_serialized_reply_fields() {
        let schemas = method_schemas::<Params, Params>();
        assert!(schemas["params_schema"].get("required").is_none());
        assert_eq!(schemas["params_schema"]["additionalProperties"], false);
        assert_eq!(schemas["result_schema"]["required"], json!(["count"]));
    }
}
