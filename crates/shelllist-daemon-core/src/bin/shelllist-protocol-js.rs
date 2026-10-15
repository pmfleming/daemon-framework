use std::{
    collections::BTreeMap,
    io::{self, Read},
};

use serde::Serialize;
use serde_json::Value;
use shelllist_daemon_core::registry_names;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() -> Result<()> {
    let mut source = String::new();
    io::stdin().read_to_string(&mut source)?;
    let value: Value = serde_json::from_str(&source)?;
    print!("{}", render(&value)?);
    Ok(())
}

fn render(value: &Value) -> Result<String> {
    let protocol = value["protocol"]
        .as_str()
        .ok_or("protocol registry is missing protocol")?;
    let version = value["version"]
        .as_u64()
        .ok_or("protocol registry is missing version")?;
    let registry = value
        .get("registry")
        .or_else(|| value.pointer("/data/protocol"))
        .ok_or("protocol registry is missing registry data")?;
    let methods = registry_names(registry, "methods")?;
    let streams = registry_names(registry, "streams")?;

    let mut output = format!(
        ".pragma library\n\n// Generated from the daemon-owned protocol registry. Do not edit.\nvar protocol = {};\nvar version = {version};\n{}{}",
        serde_json::to_string(protocol)?,
        render_names("methods", &methods)?,
        render_names("streams", &streams)?,
    );
    let contracts = method_contracts(registry)?;
    let events = event_contracts(registry)?;
    if !contracts.is_empty() || !events.is_empty() {
        output.push_str(&format!(
            "\nvar contracts = {};\nvar eventContracts = {};\n",
            serde_json::to_string(&contracts)?,
            serde_json::to_string(&events)?
        ));
        output.push_str(include_str!("../schema_validation.js"));
    }
    Ok(output)
}

#[derive(Serialize)]
struct MethodContract<'a> {
    params_schema: &'a Value,
    policy: &'a Value,
    response_key: &'a Value,
    result_schema: &'a Value,
}

// Borrow schema trees all the way to serialization; no duplicate JSON documents.
fn method_contracts(registry: &Value) -> Result<BTreeMap<&str, MethodContract<'_>>> {
    let mut contracts = BTreeMap::new();
    for method in registry["methods"]
        .as_array()
        .ok_or("methods must be an array")?
    {
        let (Some(params_schema), Some(result_schema)) =
            (method.get("params_schema"), method.get("result_schema"))
        else {
            continue;
        };
        check_schema(params_schema)?;
        check_schema(result_schema)?;
        contracts.insert(
            method["name"].as_str().ok_or("missing method name")?,
            MethodContract {
                params_schema,
                result_schema,
                response_key: &method["response_key"],
                policy: &method["policy"],
            },
        );
    }
    Ok(contracts)
}

fn event_contracts(registry: &Value) -> Result<BTreeMap<String, &Value>> {
    let mut events = BTreeMap::new();
    for stream in registry["streams"]
        .as_array()
        .ok_or("streams must be an array")?
    {
        for entry in stream["events"].as_array().into_iter().flatten() {
            let Some(schema) = entry.get("data_schema") else {
                continue;
            };
            check_schema(schema)?;
            let key = format!(
                "{}:{}",
                stream["name"].as_str().ok_or("stream name")?,
                entry["name"].as_str().ok_or("event name")?
            );
            events.insert(key, schema);
        }
    }
    Ok(events)
}

// Never silently ignore a newly generated assertion unsupported by QML validation.
fn check_schema(schema: &Value) -> Result<()> {
    if schema.is_boolean() {
        return Ok(());
    }
    let object = schema
        .as_object()
        .ok_or("schema must be boolean or object")?;
    for (key, value) in object {
        match key.as_str() {
            "$defs" | "properties" => value
                .as_object()
                .ok_or("schema map must be an object")?
                .values()
                .try_for_each(check_schema)?,
            "allOf" | "anyOf" | "oneOf" => value
                .as_array()
                .ok_or("schema alternatives must be an array")?
                .iter()
                .try_for_each(check_schema)?,
            "items" | "additionalProperties" | "not" => check_schema(value)?,
            "$ref"
                if !value
                    .as_str()
                    .is_some_and(|reference| reference.starts_with("#/")) =>
            {
                return Err("only local schema references are supported".into());
            }
            "$schema" | "$id" | "$ref" | "title" | "description" | "default" | "examples"
            | "deprecated" | "readOnly" | "writeOnly" | "format" | "type" | "const" | "enum"
            | "required" | "minimum" | "maximum" | "exclusiveMinimum" | "exclusiveMaximum"
            | "minLength" | "maxLength" | "pattern" | "minItems" | "maxItems" => (),
            _ => return Err(format!("unsupported QML schema keyword: {key}").into()),
        }
    }
    Ok(())
}

fn render_names(variable: &str, names: &[&str]) -> Result<String> {
    let mut output = format!("var {variable} = ({{\n");
    for name in names {
        let encoded = serde_json::to_string(name)?;
        output.push_str(&format!("    {encoded}: {encoded},\n"));
    }
    output.push_str("});\n");
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{check_schema, render};
    use serde_json::json;
    #[test]
    fn generation_rejects_unsupported_assertions_and_external_references() {
        assert!(check_schema(&json!({"type":"array", "uniqueItems":true})).is_err());
        assert!(check_schema(&json!({"$ref":"https://example.invalid/schema"})).is_err());
        assert!(check_schema(&json!({"properties":{"nested":{"multipleOf":2}}})).is_err());
        assert!(
            check_schema(&json!({"type":["string","null"], "$defs":{"x":{"type":"integer"}}}))
                .is_ok()
        );
    }

    #[test]
    fn generation_preserves_optional_contracts_and_sorted_keys() {
        let output = render(&json!({"protocol":"test", "version":2, "registry":{
            "methods":[{"name":"bare"}, {"name":"typed", "params_schema":true, "result_schema":false}],
            "streams":[{"name":"updates", "events":[{"name":"changed", "data_schema":true}]}]
        }})).unwrap();
        assert!(output.contains("var contracts = {\"typed\":{\"params_schema\":true,\"policy\":null,\"response_key\":null,\"result_schema\":false}};"));
        assert!(output.contains("var eventContracts = {\"updates:changed\":true};"));
    }
}
