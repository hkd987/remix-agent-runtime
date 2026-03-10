use crate::llm::types::ToolDefinition;
use serde_json::Value;

/// Represents an MCP tool as returned by the MCP server's list_tools.
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Convert MCP tools to Anthropic tool definitions.
pub fn convert_mcp_tools(tools: &[McpToolInfo]) -> Vec<ToolDefinition> {
    tools
        .iter()
        .map(|t| ToolDefinition {
            name: t.name.clone(),
            description: t.description.clone(),
            input_schema: t.input_schema.clone(),
            cache_control: None,
            read_only: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_convert_single_tool() {
        let mcp_tools = vec![McpToolInfo {
            name: "navigate".to_string(),
            description: "Navigate to a URL".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"}
                },
                "required": ["url"]
            }),
        }];

        let result = convert_mcp_tools(&mcp_tools);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "navigate");
        assert_eq!(result[0].description, "Navigate to a URL");
        assert_eq!(
            result[0].input_schema,
            json!({
                "type": "object",
                "properties": {
                    "url": {"type": "string"}
                },
                "required": ["url"]
            })
        );
    }

    #[test]
    fn test_convert_multiple_tools() {
        let mcp_tools = vec![
            McpToolInfo {
                name: "navigate".to_string(),
                description: "Navigate to a URL".to_string(),
                input_schema: json!({"type": "object", "properties": {"url": {"type": "string"}}}),
            },
            McpToolInfo {
                name: "click".to_string(),
                description: "Click an element".to_string(),
                input_schema: json!({"type": "object", "properties": {"selector": {"type": "string"}}}),
            },
            McpToolInfo {
                name: "screenshot".to_string(),
                description: "Take a screenshot".to_string(),
                input_schema: json!({"type": "object", "properties": {}}),
            },
        ];

        let result = convert_mcp_tools(&mcp_tools);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].name, "navigate");
        assert_eq!(result[1].name, "click");
        assert_eq!(result[2].name, "screenshot");
    }

    #[test]
    fn test_convert_empty_tools() {
        let mcp_tools: Vec<McpToolInfo> = vec![];
        let result = convert_mcp_tools(&mcp_tools);
        assert!(result.is_empty());
    }

    #[test]
    fn test_convert_complex_schema() {
        let mcp_tools = vec![McpToolInfo {
            name: "fill_form".to_string(),
            description: "Fill a form field with a value".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for the form field"
                    },
                    "value": {
                        "type": "string",
                        "description": "Value to fill in"
                    },
                    "options": {
                        "type": "object",
                        "properties": {
                            "clear_first": {"type": "boolean", "default": true},
                            "delay_ms": {"type": "integer", "minimum": 0}
                        }
                    }
                },
                "required": ["selector", "value"],
                "additionalProperties": false
            }),
        }];

        let result = convert_mcp_tools(&mcp_tools);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "fill_form");

        let schema = &result[0].input_schema;
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["properties"]["selector"]["type"], "string");
        assert_eq!(schema["properties"]["options"]["type"], "object");
        assert_eq!(schema["required"][0], "selector");
        assert_eq!(schema["required"][1], "value");
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn test_convert_preserves_all_fields() {
        let mcp_tool = McpToolInfo {
            name: "test_tool".to_string(),
            description: "A test tool description".to_string(),
            input_schema: json!({"type": "object"}),
        };

        let result = convert_mcp_tools(&[mcp_tool.clone()]);
        assert_eq!(result[0].name, mcp_tool.name);
        assert_eq!(result[0].description, mcp_tool.description);
        assert_eq!(result[0].input_schema, mcp_tool.input_schema);
    }
}
