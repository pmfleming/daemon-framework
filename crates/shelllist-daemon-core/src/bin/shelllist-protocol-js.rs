use std::io::{self, Read};

use serde_json::Value;

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
    let methods = names(registry, "methods")?;
    let streams = names(registry, "streams")?;

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
    Ok(output)
}

fn names(registry: &Value, field: &str) -> Result<Vec<String>> {
    registry
        .get(field)
        .and_then(Value::as_array)
        .ok_or_else(|| io::Error::other(format!("protocol registry is missing {field}")))?
        .iter()
        .map(|entry| {
            entry
                .get("name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| {
                    io::Error::other(format!("protocol {field} entry is missing name")).into()
                })
        })
        .collect()
}

fn render_names(variable: &str, names: &[String]) -> Result<String> {
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
    use serde_json::json;

    use super::render;

    #[test]
    fn renders_direct_and_enveloped_registries() {
        let direct = json!({
            "protocol": "test-api",
            "version": 2,
            "registry": {
                "methods": [{ "name": "thing.read" }],
                "streams": [{ "name": "thing.changed" }]
            }
        });
        let output = render(&direct).unwrap();
        assert!(output.contains("var protocol = \"test-api\";"));
        assert!(output.contains("\"thing.read\": \"thing.read\""));

        let enveloped = json!({
            "protocol": "test-api",
            "version": 2,
            "data": { "protocol": direct["registry"].clone() }
        });
        assert_eq!(render(&enveloped).unwrap(), output);
    }
}
