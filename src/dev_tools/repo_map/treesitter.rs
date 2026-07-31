//! Tree-sitter symbol extraction for the repo map.
//!
//! The `dev-tools` Cargo feature pulled in five tree-sitter crates and declared it
//! provided "code intelligence via tree-sitter parsing", but there was not a single
//! `#[cfg(feature = "dev-tools")]` anywhere in the tree: symbol extraction was entirely
//! regex-based, and enabling the feature changed nothing but the dependency graph.
//!
//! This is the real implementation. The regex extractors remain as the fallback when
//! the feature is off, so behaviour degrades rather than disappearing.

use tree_sitter::{Node, Parser};

use super::types::{SymbolEntry, SymbolKind};

/// A language the repo map can parse with tree-sitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
}

impl Language {
    /// Map a file extension to a parser, or `None` if unsupported.
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext {
            "rs" => Some(Self::Rust),
            "ts" => Some(Self::TypeScript),
            "tsx" | "jsx" => Some(Self::Tsx),
            "js" | "mjs" | "cjs" => Some(Self::JavaScript),
            "py" | "pyi" => Some(Self::Python),
            _ => None,
        }
    }

    fn ts_language(&self) -> tree_sitter::Language {
        match self {
            Self::Rust => tree_sitter_rust::LANGUAGE.into(),
            Self::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Self::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            Self::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            Self::Python => tree_sitter_python::LANGUAGE.into(),
        }
    }

    /// Map a node kind produced by this grammar to the symbol kind it represents.
    ///
    /// Driven off node kinds rather than a query string so no extra streaming-iterator
    /// dependency is needed, and so an unknown construct is simply skipped.
    fn symbol_kind(&self, node_kind: &str) -> Option<SymbolKind> {
        match self {
            Self::Rust => Some(match node_kind {
                "function_item" => SymbolKind::Function,
                "struct_item" => SymbolKind::Struct,
                "enum_item" => SymbolKind::Enum,
                "trait_item" => SymbolKind::Trait,
                "type_item" => SymbolKind::Type,
                "const_item" | "static_item" => SymbolKind::Constant,
                "mod_item" => SymbolKind::Module,
                _ => return None,
            }),
            Self::TypeScript | Self::Tsx => Some(match node_kind {
                "function_declaration" => SymbolKind::Function,
                "class_declaration" => SymbolKind::Class,
                "interface_declaration" => SymbolKind::Interface,
                "type_alias_declaration" => SymbolKind::Type,
                "enum_declaration" => SymbolKind::Enum,
                "method_definition" => SymbolKind::Method,
                _ => return None,
            }),
            Self::JavaScript => Some(match node_kind {
                "function_declaration" => SymbolKind::Function,
                "class_declaration" => SymbolKind::Class,
                "method_definition" => SymbolKind::Method,
                _ => return None,
            }),
            Self::Python => Some(match node_kind {
                "function_definition" => SymbolKind::Function,
                "class_definition" => SymbolKind::Class,
                _ => return None,
            }),
        }
    }
}

/// Extract symbols from `content` using tree-sitter.
///
/// Returns `None` when the language is unsupported or the parser could not be
/// configured, so the caller can fall back to the regex extractors rather than
/// silently producing an empty map.
pub fn extract_symbols(content: &str, ext: &str) -> Option<Vec<SymbolEntry>> {
    let language = Language::from_extension(ext)?;
    let ts_language = language.ts_language();

    let mut parser = Parser::new();
    if parser.set_language(&ts_language).is_err() {
        tracing::debug!(ext = %ext, "Failed to configure tree-sitter parser");
        return None;
    }

    let tree = parser.parse(content, None)?;

    let mut symbols = Vec::new();
    collect_symbols(tree.root_node(), content, language, &mut symbols);

    // Nodes are visited depth-first, so nested items can land out of order; a repo map
    // reads far better top-to-bottom.
    symbols.sort_by_key(|s| s.line);
    Some(symbols)
}

/// Walk the tree, recording every node whose kind maps to a symbol.
fn collect_symbols(node: Node, content: &str, language: Language, out: &mut Vec<SymbolEntry>) {
    if let Some(kind) = language.symbol_kind(node.kind()) {
        if let Some(name_node) = node.child_by_field_name("name") {
            if let Ok(name) = name_node.utf8_text(content.as_bytes()) {
                out.push(SymbolEntry {
                    kind,
                    name: name.to_string(),
                    signature: signature_of(node, content),
                    line: name_node.start_position().row + 1,
                    visibility: visibility_of(node, content, language),
                });
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_symbols(child, content, language, out);
    }
}

/// The declaration's first line, trimmed and stripped of a trailing brace or colon.
fn signature_of(node: Node, content: &str) -> Option<String> {
    let text = node.utf8_text(content.as_bytes()).ok()?;
    let first_line = text.lines().next()?.trim();
    let cleaned = first_line
        .trim_end_matches('{')
        .trim_end_matches(':')
        .trim_end();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

fn visibility_of(node: Node, content: &str, language: Language) -> Option<String> {
    match language {
        Language::Rust => {
            let text = node.utf8_text(content.as_bytes()).ok()?;
            text.trim_start()
                .starts_with("pub")
                .then(|| "pub".to_string())
        }
        Language::Python => None,
        _ => {
            // `export` is the closest analogue in the JS/TS family, and it sits on the
            // statement wrapping the declaration.
            let parent = node.parent()?;
            let text = parent.utf8_text(content.as_bytes()).ok()?;
            text.trim_start()
                .starts_with("export")
                .then(|| "export".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(symbols: &[SymbolEntry]) -> Vec<&str> {
        symbols.iter().map(|s| s.name.as_str()).collect()
    }

    #[test]
    fn extracts_rust_symbols() {
        let src = r#"
pub struct Config { field: u32 }
pub enum Mode { A, B }
pub trait Runner { fn run(&self); }
pub fn main() {}
fn helper() {}
const LIMIT: u32 = 5;
pub mod inner {}
"#;
        let symbols = extract_symbols(src, "rs").expect("rust is supported");
        let got = names(&symbols);
        for expected in [
            "Config", "Mode", "Runner", "main", "helper", "LIMIT", "inner",
        ] {
            assert!(got.contains(&expected), "missing {expected}: {got:?}");
        }
    }

    #[test]
    fn records_rust_visibility() {
        let src = "pub fn exported() {}\nfn private() {}\n";
        let symbols = extract_symbols(src, "rs").unwrap();
        let exported = symbols.iter().find(|s| s.name == "exported").unwrap();
        let private = symbols.iter().find(|s| s.name == "private").unwrap();
        assert_eq!(exported.visibility.as_deref(), Some("pub"));
        assert_eq!(private.visibility, None);
    }

    #[test]
    fn extracts_typescript_symbols() {
        let src = r#"
export interface Options { a: number }
export class Service {}
export type Alias = string;
export function run(): void {}
"#;
        let symbols = extract_symbols(src, "ts").expect("typescript is supported");
        let got = names(&symbols);
        for expected in ["Options", "Service", "Alias", "run"] {
            assert!(got.contains(&expected), "missing {expected}: {got:?}");
        }
    }

    #[test]
    fn extracts_python_symbols() {
        let src = "class Handler:\n    def handle(self):\n        pass\n\ndef main():\n    pass\n";
        let symbols = extract_symbols(src, "py").expect("python is supported");
        let got = names(&symbols);
        assert!(got.contains(&"Handler"), "{got:?}");
        assert!(got.contains(&"main"), "{got:?}");
    }

    #[test]
    fn extracts_javascript_symbols() {
        let src = "export class Widget {}\nfunction build() {}\n";
        let symbols = extract_symbols(src, "js").expect("javascript is supported");
        let got = names(&symbols);
        assert!(got.contains(&"Widget"), "{got:?}");
        assert!(got.contains(&"build"), "{got:?}");
    }

    #[test]
    fn symbols_are_sorted_by_line() {
        let src = "fn a() {}\nstruct B {}\nfn c() {}\n";
        let symbols = extract_symbols(src, "rs").unwrap();
        let lines: Vec<usize> = symbols.iter().map(|s| s.line).collect();
        let mut sorted = lines.clone();
        sorted.sort_unstable();
        assert_eq!(lines, sorted, "symbols were not in source order");
    }

    #[test]
    fn unsupported_extension_returns_none() {
        // `None` rather than an empty vec, so the caller falls back to regex rather
        // than reporting a file as having no symbols.
        assert!(extract_symbols("some content", "txt").is_none());
    }

    #[test]
    fn malformed_source_does_not_panic() {
        // Tree-sitter is error-tolerant; a partial parse must still return.
        let symbols = extract_symbols("pub fn broken( {{{", "rs");
        assert!(symbols.is_some());
    }
}
