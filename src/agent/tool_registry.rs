use std::collections::HashSet;

use serde_json::json;

use crate::llm::types::ToolDefinition;

/// Registry that supports lazy tool discovery.
/// When lazy mode is active, only discovered + always-available tools are exposed to the LLM.
pub struct ToolRegistry {
    all_tools: Vec<ToolDefinition>,
    discovered: HashSet<String>,
    always_available: HashSet<String>,
}

impl ToolRegistry {
    /// Create a new registry from the full set of tool definitions.
    /// `always_available` contains tool names that are always visible (local tools).
    pub fn new(all_tools: Vec<ToolDefinition>, always_available: Vec<String>) -> Self {
        let always_set: HashSet<String> = always_available.into_iter().collect();
        Self {
            all_tools,
            discovered: HashSet::new(),
            always_available: always_set,
        }
    }

    /// Search tools by query string. Returns matching tools and marks them as discovered.
    /// Scoring: case-insensitive substring match on name (score 10) and description (score 5),
    /// plus word overlap bonus (score 2 per matching word).
    pub fn search(&mut self, query: &str, max_results: usize) -> Vec<ToolDefinition> {
        let query_lower = query.to_lowercase();
        let query_words: HashSet<&str> = query_lower.split_whitespace().collect();

        let mut scored: Vec<(usize, &ToolDefinition)> = self
            .all_tools
            .iter()
            .filter(|t| !self.always_available.contains(&t.name))
            .filter_map(|tool| {
                let name_lower = tool.name.to_lowercase();
                let desc_lower = tool.description.to_lowercase();
                let mut score = 0usize;

                // Exact name match
                if name_lower == query_lower {
                    score += 20;
                }
                // Name contains query
                if name_lower.contains(&query_lower) {
                    score += 10;
                }
                // Description contains query
                if desc_lower.contains(&query_lower) {
                    score += 5;
                }
                // Word overlap in name and description
                let tool_words: HashSet<&str> = name_lower
                    .split(|c: char| !c.is_alphanumeric())
                    .chain(desc_lower.split_whitespace())
                    .collect();
                for qw in &query_words {
                    if tool_words.iter().any(|tw| tw.contains(qw)) {
                        score += 2;
                    }
                }

                if score > 0 {
                    Some((score, tool))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        let results: Vec<ToolDefinition> = scored
            .into_iter()
            .take(max_results)
            .map(|(_, t)| t.clone())
            .collect();

        // Mark as discovered
        for tool in &results {
            self.discovered.insert(tool.name.clone());
        }

        results
    }

    /// Return tool definitions that should be sent to the LLM in lazy mode.
    /// Includes always-available tools + discovered tools + search_tools meta-tool.
    pub fn discovered_definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self
            .all_tools
            .iter()
            .filter(|t| {
                self.always_available.contains(&t.name) || self.discovered.contains(&t.name)
            })
            .cloned()
            .collect();
        defs.push(Self::search_tool_definition());
        defs
    }

    /// Return all tool definitions (for non-lazy mode).
    pub fn all_definitions(&self) -> &[ToolDefinition] {
        &self.all_tools
    }

    /// Return the ToolDefinition for the search_tools meta-tool.
    pub fn search_tool_definition() -> ToolDefinition {
        ToolDefinition {
            name: "search_tools".to_string(),
            description: "Search for available tools by keyword. Use this when you need a \
                          capability that isn't currently available. Returns matching tool names \
                          and descriptions."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Keywords describing the capability you need (e.g., 'navigate browser', 'click element', 'take screenshot')"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of tools to return (default: 5)",
                        "default": 5
                    }
                },
                "required": ["query"]
            }),
            cache_control: None,
            read_only: false,
        }
    }

    /// Check if a tool name is the search_tools meta-tool.
    pub fn is_search_tool(name: &str) -> bool {
        name == "search_tools"
    }

    /// Format search results as a human-readable string for the tool result.
    pub fn format_search_results(results: &[ToolDefinition]) -> String {
        if results.is_empty() {
            return "No matching tools found. Try different keywords.".to_string();
        }
        let mut output = format!("Found {} matching tool(s):\n\n", results.len());
        for tool in results {
            output.push_str(&format!("- **{}**: {}\n", tool.name, tool.description));
        }
        output.push_str("\nThese tools are now available for use.");
        output
    }

    /// Get the number of discovered tools.
    pub fn discovered_count(&self) -> usize {
        self.discovered.len()
    }

    /// Get the total number of tools (excluding always-available) that are undiscovered.
    pub fn undiscovered_count(&self) -> usize {
        self.all_tools
            .iter()
            .filter(|t| {
                !self.always_available.contains(&t.name) && !self.discovered.contains(&t.name)
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tool(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition {
            name: name.to_string(),
            description: description.to_string(),
            input_schema: json!({"type": "object"}),
            cache_control: None,
            read_only: false,
        }
    }

    fn sample_tools() -> Vec<ToolDefinition> {
        vec![
            make_tool("navigate", "Navigate the browser to a URL"),
            make_tool("click", "Click an element on the page"),
            make_tool("screenshot", "Take a screenshot of the browser page"),
            make_tool("type_text", "Type text into a form field"),
            make_tool("read_file", "Read a file from the filesystem"),
            make_tool("write_file", "Write content to a file"),
        ]
    }

    fn sample_always_available() -> Vec<String> {
        vec!["read_file".to_string(), "write_file".to_string()]
    }

    #[test]
    fn test_new_creates_empty_discovered() {
        let registry = ToolRegistry::new(sample_tools(), sample_always_available());
        assert_eq!(registry.discovered_count(), 0);
    }

    #[test]
    fn test_search_finds_by_name() {
        let mut registry = ToolRegistry::new(sample_tools(), sample_always_available());
        let results = registry.search("navigate", 5);
        assert!(!results.is_empty());
        assert!(results.iter().any(|t| t.name == "navigate"));
    }

    #[test]
    fn test_search_finds_by_description() {
        let mut registry = ToolRegistry::new(sample_tools(), sample_always_available());
        let results = registry.search("browser", 5);
        assert!(!results.is_empty());
        // "navigate" and "screenshot" both mention "browser" in description
        let names: Vec<&str> = results.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"navigate"));
        assert!(names.contains(&"screenshot"));
    }

    #[test]
    fn test_search_marks_as_discovered() {
        let mut registry = ToolRegistry::new(sample_tools(), sample_always_available());
        assert_eq!(registry.discovered_count(), 0);

        let results = registry.search("navigate", 5);
        assert!(!results.is_empty());
        assert!(registry.discovered_count() > 0);

        // The discovered definitions should now include navigate
        let defs = registry.discovered_definitions();
        assert!(defs.iter().any(|t| t.name == "navigate"));
    }

    #[test]
    fn test_search_respects_max_results() {
        let mut registry = ToolRegistry::new(sample_tools(), sample_always_available());
        // "the" or "a" are common enough but let's search for something broad
        let results = registry.search("a", 2);
        assert!(results.len() <= 2);
    }

    #[test]
    fn test_search_no_match_returns_empty() {
        let mut registry = ToolRegistry::new(sample_tools(), sample_always_available());
        let results = registry.search("zzz_nonexistent_zzz", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_discovered_definitions_includes_always_available() {
        let registry = ToolRegistry::new(sample_tools(), sample_always_available());
        let defs = registry.discovered_definitions();
        assert!(defs.iter().any(|t| t.name == "read_file"));
        assert!(defs.iter().any(|t| t.name == "write_file"));
    }

    #[test]
    fn test_discovered_definitions_includes_search_tool() {
        let registry = ToolRegistry::new(sample_tools(), sample_always_available());
        let defs = registry.discovered_definitions();
        assert!(defs.iter().any(|t| t.name == "search_tools"));
    }

    #[test]
    fn test_all_definitions_returns_everything() {
        let tools = sample_tools();
        let total = tools.len();
        let registry = ToolRegistry::new(tools, sample_always_available());
        assert_eq!(registry.all_definitions().len(), total);
    }

    #[test]
    fn test_search_tool_definition_has_required_fields() {
        let def = ToolRegistry::search_tool_definition();
        assert_eq!(def.name, "search_tools");
        assert!(!def.description.is_empty());

        // Validate input_schema structure
        let schema = &def.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["query"].is_object());
        assert_eq!(schema["properties"]["query"]["type"], "string");
        assert!(schema["properties"]["max_results"].is_object());
        assert_eq!(schema["properties"]["max_results"]["type"], "integer");

        let required = schema["required"]
            .as_array()
            .expect("required should be an array");
        let required_strs: Vec<&str> = required
            .iter()
            .map(|v| v.as_str().expect("required item should be a string"))
            .collect();
        assert!(required_strs.contains(&"query"));
    }

    #[test]
    fn test_format_search_results_empty() {
        let output = ToolRegistry::format_search_results(&[]);
        assert_eq!(output, "No matching tools found. Try different keywords.");
    }

    #[test]
    fn test_format_search_results_with_tools() {
        let tools = vec![
            make_tool("navigate", "Navigate to a URL"),
            make_tool("click", "Click an element"),
        ];
        let output = ToolRegistry::format_search_results(&tools);
        assert!(output.contains("Found 2 matching tool(s):"));
        assert!(output.contains("**navigate**"));
        assert!(output.contains("**click**"));
        assert!(output.contains("These tools are now available for use."));
    }

    #[test]
    fn test_is_search_tool() {
        assert!(ToolRegistry::is_search_tool("search_tools"));
        assert!(!ToolRegistry::is_search_tool("navigate"));
        assert!(!ToolRegistry::is_search_tool("search_tool"));
        assert!(!ToolRegistry::is_search_tool(""));
    }

    #[test]
    fn test_search_excludes_always_available() {
        let mut registry = ToolRegistry::new(sample_tools(), sample_always_available());
        // "file" appears in both read_file and write_file (always-available)
        let results = registry.search("file", 10);
        let names: Vec<&str> = results.iter().map(|t| t.name.as_str()).collect();
        assert!(!names.contains(&"read_file"));
        assert!(!names.contains(&"write_file"));
    }

    #[test]
    fn test_discovered_count_and_undiscovered_count() {
        let mut registry = ToolRegistry::new(sample_tools(), sample_always_available());
        // 6 total tools, 2 always-available => 4 discoverable
        assert_eq!(registry.discovered_count(), 0);
        assert_eq!(registry.undiscovered_count(), 4);

        registry.search("navigate", 5);
        // "navigate" should be discovered now
        assert!(registry.discovered_count() >= 1);
        let undiscovered = registry.undiscovered_count();
        assert_eq!(
            registry.discovered_count() + undiscovered,
            4,
            "discovered + undiscovered should equal total discoverable tools"
        );
    }

    #[test]
    fn test_search_scoring_exact_name_ranks_highest() {
        let tools = vec![
            make_tool("click", "Click an element on the page"),
            make_tool("click_element", "Click a specific element by selector"),
            make_tool("double_click", "Double-click an element"),
        ];
        let mut registry = ToolRegistry::new(tools, vec![]);
        let results = registry.search("click", 5);

        assert!(!results.is_empty());
        // The exact match "click" should be first
        assert_eq!(
            results[0].name, "click",
            "Exact name match should rank highest"
        );
    }
}
