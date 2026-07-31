//! Typed argument structs for the local tools.
//!
//! Every tool previously re-derived its arguments from a raw `Value` with a chain of
//! `.get(..).and_then(as_str).ok_or_else(..)`, repeated ~35 times across the tree with
//! inconsistent error wording, while its JSON Schema lived separately in
//! `local_tools::executor` and was neither derived from nor checked against the parsing
//! code. Adding a parameter meant editing two places, and they could drift silently.
//!
//! These structs are the parsing half. `tests::schema_matches_parser` below is what
//! keeps the two halves honest.
//!
//! They use `deny_unknown_fields` for two reasons: it makes that drift test able to
//! detect a schema property the parser does not know about (serde ignores unknown
//! fields by default, so without it the check passes vacuously), and it gives the model
//! a clear error when it invents a parameter instead of silently dropping it.

use serde::Deserialize;

use crate::error::AgentError;

/// Deserialize tool arguments, reporting the tool name in any error.
pub fn parse<T: serde::de::DeserializeOwned>(
    tool: &str,
    arguments: serde_json::Value,
) -> Result<T, AgentError> {
    serde_json::from_value(arguments)
        .map_err(|e| AgentError::LocalTool(format!("Invalid arguments for {tool}: {e}")))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadFileArgs {
    pub path: String,
    /// Line to start from, 0-indexed.
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditFileArgs {
    pub path: String,
    pub old_string: String,
    pub new_string: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BashArgs {
    pub command: String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrepArgs {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub include: Option<String>,
    #[serde(default)]
    pub context_lines: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GlobArgs {
    pub pattern: String,
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebFetchArgs {
    pub url: String,
    #[serde(default)]
    pub headers: Option<std::collections::HashMap<String, String>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    #[test]
    fn parse_reports_the_tool_name() {
        let err = parse::<ReadFileArgs>("read_file", json!({})).unwrap_err();
        assert!(
            err.to_string().contains("Invalid arguments for read_file"),
            "{err}"
        );
    }

    #[test]
    fn optional_fields_may_be_omitted() {
        let args: ReadFileArgs = parse("read_file", json!({"path": "a.txt"})).unwrap();
        assert_eq!(args.path, "a.txt");
        assert_eq!(args.offset, None);
        assert_eq!(args.limit, None);
    }

    /// A placeholder value of the right JSON type for a schema property.
    fn sample_for(schema: &Value) -> Value {
        match schema.get("type").and_then(|t| t.as_str()) {
            Some("integer") | Some("number") => json!(1),
            Some("boolean") => json!(true),
            Some("array") => json!([]),
            Some("object") => json!({}),
            _ => json!("x"),
        }
    }

    /// Every declared schema must agree with the struct that parses it.
    ///
    /// This is the check that was missing: the schema in `local_tools::executor` and the
    /// parsing code were written independently, so a parameter added to one and not the
    /// other produced a tool the model could call but not use.
    #[test]
    fn schema_matches_parser() {
        // (tool name, parser) for every tool whose arguments are declared by a schema.
        type Parser = fn(Value) -> Result<(), AgentError>;
        let parsers: Vec<(&str, Parser)> = vec![
            ("read_file", |v| {
                parse::<ReadFileArgs>("read_file", v).map(|_| ())
            }),
            ("write_file", |v| {
                parse::<WriteFileArgs>("write_file", v).map(|_| ())
            }),
            ("edit_file", |v| {
                parse::<EditFileArgs>("edit_file", v).map(|_| ())
            }),
            ("bash", |v| parse::<BashArgs>("bash", v).map(|_| ())),
            ("grep", |v| parse::<GrepArgs>("grep", v).map(|_| ())),
            ("glob", |v| parse::<GlobArgs>("glob", v).map(|_| ())),
            ("web_fetch", |v| {
                parse::<WebFetchArgs>("web_fetch", v).map(|_| ())
            }),
        ];

        let defs = crate::local_tools::executor::local_tool_definitions();

        for (name, parse_fn) in parsers {
            let def = defs
                .iter()
                .find(|d| d.name == name)
                .unwrap_or_else(|| panic!("no tool definition named `{name}`"));

            let props = def.input_schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("`{name}` schema has no properties"));
            let required: Vec<&str> = def.input_schema["required"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            // Every property the schema advertises must be accepted by the parser.
            let mut full = serde_json::Map::new();
            for (prop, prop_schema) in props {
                full.insert(prop.clone(), sample_for(prop_schema));
            }
            parse_fn(Value::Object(full.clone())).unwrap_or_else(|e| {
                panic!("`{name}` parser rejected its own declared schema: {e}")
            });

            // Every field the schema marks required must actually be required.
            for req in &required {
                let mut without = full.clone();
                without.remove(*req);
                assert!(
                    parse_fn(Value::Object(without)).is_err(),
                    "`{name}` schema lists `{req}` as required but the parser accepts it missing"
                );
            }

            // And nothing else should be, or the schema is under-specified.
            for prop in props.keys() {
                if required.contains(&prop.as_str()) {
                    continue;
                }
                let mut without = full.clone();
                without.remove(prop);
                assert!(
                    parse_fn(Value::Object(without)).is_ok(),
                    "`{name}` parser requires `{prop}` but the schema does not list it as required"
                );
            }
        }
    }
}
