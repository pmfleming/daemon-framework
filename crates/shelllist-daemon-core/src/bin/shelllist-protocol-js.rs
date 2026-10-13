use std::io::{self, Read};

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
    let protocol = value
        .get("protocol")
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other("protocol registry is missing protocol"))?;
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| io::Error::other("protocol registry is missing version"))?;
    let registry = value
        .get("registry")
        .or_else(|| value.pointer("/data/protocol"))
        .ok_or_else(|| io::Error::other("protocol registry is missing registry data"))?;
    let methods = registry_names(registry, "methods").map_err(io::Error::other)?;
    let streams = registry_names(registry, "streams").map_err(io::Error::other)?;

    let mut output = String::from(
        ".pragma library\n\n// Generated from the daemon-owned protocol registry. Do not edit.\n",
    );
    output.push_str(&format!(
        "var protocol = {};\n",
        serde_json::to_string(protocol)?
    ));
    output.push_str(&format!("var version = {version};\n"));
    output.push_str(&render_names("methods", &methods)?);
    output.push_str(&render_names("streams", &streams)?);
    let mut contracts = serde_json::Map::new();
    for method in registry["methods"]
        .as_array()
        .ok_or("methods must be an array")?
    {
        if let (Some(params), Some(result)) =
            (method.get("params_schema"), method.get("result_schema"))
        {
            check_schema(params)?;
            check_schema(result)?;
            let name = method["name"].as_str().ok_or("missing method name")?;
            contracts.insert(
                name.into(),
                serde_json::json!({
                    "params_schema": params, "result_schema": result,
                    "response_key": method["response_key"], "policy": method["policy"],
                }),
            );
        }
    }
    if !contracts.is_empty() {
        output.push_str(&format!(
            "\nvar contracts = {};\n",
            serde_json::to_string(&contracts)?
        ));
        output.push_str(include_str!("../schema_validation.js"));
    }
    Ok(output)
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
            "$defs" | "properties" => {
                for child in value
                    .as_object()
                    .ok_or("schema map must be an object")?
                    .values()
                {
                    check_schema(child)?;
                }
            }
            "allOf" | "anyOf" | "oneOf" => {
                for child in value
                    .as_array()
                    .ok_or("schema alternatives must be an array")?
                {
                    check_schema(child)?;
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;
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
