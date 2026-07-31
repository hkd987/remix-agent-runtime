use std::path::{Path, PathBuf};

use crate::browser::mcp::ToolExecutionResult;
use crate::config::schema::RepoMapConfig;
use crate::error::AgentError;
use crate::local_tools::tools::output_filter::truncate_output;

use super::types::{RepoMapEntry, SymbolEntry, SymbolKind};

const MAX_OUTPUT_BYTES: usize = 65_536; // 64KB cap

pub async fn execute_repo_map(
    arguments: serde_json::Value,
    config: &RepoMapConfig,
) -> Result<ToolExecutionResult, AgentError> {
    let path = arguments
        .get("path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    let depth = arguments
        .get("depth")
        .and_then(|v| v.as_u64())
        .map(|d| d as usize)
        .unwrap_or(config.max_depth);

    let include = arguments
        .get("include")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let symbols = arguments
        .get("symbols")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let entries = generate_repo_map(&path, depth, config.max_files, include.as_deref(), symbols)?;

    let output = format_repo_map(&entries, &path);

    let content = truncate_output(&output, MAX_OUTPUT_BYTES);

    Ok(ToolExecutionResult {
        content,
        is_error: false,
    })
}

fn generate_repo_map(
    root: &Path,
    max_depth: usize,
    max_files: usize,
    include: Option<&str>,
    extract_symbols: bool,
) -> Result<Vec<RepoMapEntry>, AgentError> {
    let mut entries = Vec::new();
    let mut files = Vec::new();

    collect_files(root, max_depth, 0, include, &mut files)?;

    // Sort by path for deterministic output
    files.sort();

    // Cap at max_files
    files.truncate(max_files);

    for file_path in &files {
        let relative = file_path
            .strip_prefix(root)
            .unwrap_or(file_path)
            .to_string_lossy()
            .to_string();

        let symbols = if extract_symbols {
            extract_symbols_from_file(file_path)
        } else {
            Vec::new()
        };

        entries.push(RepoMapEntry {
            file: relative,
            symbols,
        });
    }

    Ok(entries)
}

fn collect_files(
    dir: &Path,
    max_depth: usize,
    current_depth: usize,
    include: Option<&str>,
    files: &mut Vec<PathBuf>,
) -> Result<(), AgentError> {
    if current_depth > max_depth {
        return Ok(());
    }

    let entries = std::fs::read_dir(dir).map_err(|e| {
        AgentError::ToolExecution(format!("Failed to read directory {}: {}", dir.display(), e))
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| {
            AgentError::ToolExecution(format!("Failed to read directory entry: {}", e))
        })?;
        let path = entry.path();
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();

        // Skip hidden dirs and common non-source dirs
        if name.starts_with('.') || is_ignored_dir(&name) {
            continue;
        }

        if path.is_dir() {
            collect_files(&path, max_depth, current_depth + 1, include, files)?;
        } else if is_source_file(&name) {
            if let Some(pattern) = include {
                if !matches_glob(&name, pattern) {
                    continue;
                }
            }
            files.push(path);
        }
    }

    Ok(())
}

fn is_ignored_dir(name: &str) -> bool {
    matches!(
        name,
        "node_modules"
            | "target"
            | "dist"
            | "build"
            | "__pycache__"
            | ".git"
            | "vendor"
            | "venv"
            | ".venv"
            | "env"
            | ".env"
            | "coverage"
            | ".next"
            | ".nuxt"
    )
}

fn is_source_file(name: &str) -> bool {
    let extensions = [
        "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "kt", "swift", "c", "cpp", "h", "hpp",
        "cs", "rb", "ex", "exs", "zig", "lua", "vim", "sh", "bash", "zsh",
    ];
    extensions
        .iter()
        .any(|ext| name.ends_with(&format!(".{}", ext)))
}

fn matches_glob(name: &str, pattern: &str) -> bool {
    // Simple glob matching: *.rs, *.py, etc.
    if let Some(ext) = pattern.strip_prefix("*.") {
        name.ends_with(&format!(".{}", ext))
    } else {
        name.contains(pattern)
    }
}

/// Extract symbols from a source file.
///
/// With the `dev-tools` feature enabled this parses with tree-sitter, which understands
/// the grammar and so handles multi-line signatures, nested items, and anything else
/// the line-oriented regexes miss. Without it — or for a language tree-sitter is not
/// configured for — it falls back to the regex extractors below.
fn extract_symbols_from_file(path: &Path) -> Vec<SymbolEntry> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");

    #[cfg(feature = "dev-tools")]
    if let Some(symbols) = super::treesitter::extract_symbols(&content, ext) {
        return symbols;
    }

    extract_symbols_with_regex(&content, ext)
}

/// Line-oriented fallback used when tree-sitter is unavailable.
fn extract_symbols_with_regex(content: &str, ext: &str) -> Vec<SymbolEntry> {
    match ext {
        "rs" => extract_rust_symbols(content),
        "ts" | "tsx" | "js" | "jsx" => extract_typescript_symbols(content),
        "py" => extract_python_symbols(content),
        "go" => extract_go_symbols(content),
        _ => Vec::new(),
    }
}

fn extract_rust_symbols(content: &str) -> Vec<SymbolEntry> {
    let mut symbols = Vec::new();

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // pub fn / fn
        if let Some(sig) = extract_rust_fn(trimmed) {
            let vis = if trimmed.starts_with("pub") {
                Some("pub".to_string())
            } else {
                None
            };
            symbols.push(SymbolEntry {
                kind: SymbolKind::Function,
                name: sig.0,
                signature: Some(sig.1),
                line: line_idx + 1,
                visibility: vis,
            });
        }

        // pub struct / struct
        if let Some(name) = extract_keyword_name(trimmed, "struct") {
            let vis = if trimmed.starts_with("pub") {
                Some("pub".to_string())
            } else {
                None
            };
            symbols.push(SymbolEntry {
                kind: SymbolKind::Struct,
                name,
                signature: None,
                line: line_idx + 1,
                visibility: vis,
            });
        }

        // pub enum / enum
        if let Some(name) = extract_keyword_name(trimmed, "enum") {
            let vis = if trimmed.starts_with("pub") {
                Some("pub".to_string())
            } else {
                None
            };
            symbols.push(SymbolEntry {
                kind: SymbolKind::Enum,
                name,
                signature: None,
                line: line_idx + 1,
                visibility: vis,
            });
        }

        // pub trait / trait
        if let Some(name) = extract_keyword_name(trimmed, "trait") {
            let vis = if trimmed.starts_with("pub") {
                Some("pub".to_string())
            } else {
                None
            };
            symbols.push(SymbolEntry {
                kind: SymbolKind::Trait,
                name,
                signature: None,
                line: line_idx + 1,
                visibility: vis,
            });
        }

        // pub mod / mod (but not use statements)
        if (trimmed.starts_with("pub mod ") || trimmed.starts_with("mod "))
            && !trimmed.starts_with("mod tests")
        {
            if let Some(name) = extract_keyword_name(trimmed, "mod") {
                symbols.push(SymbolEntry {
                    kind: SymbolKind::Module,
                    name,
                    signature: None,
                    line: line_idx + 1,
                    visibility: if trimmed.starts_with("pub") {
                        Some("pub".to_string())
                    } else {
                        None
                    },
                });
            }
        }
    }

    symbols
}

fn extract_rust_fn(line: &str) -> Option<(String, String)> {
    // Match: pub fn name(...) or fn name(...) or pub async fn name(...)
    let line = line.trim();

    let fn_idx = if let Some(idx) = line.find("fn ") {
        // Make sure 'fn' is preceded by start, space, or keywords
        if idx == 0 || line[..idx].ends_with(' ') || line[..idx].ends_with(')') {
            idx
        } else {
            return None;
        }
    } else {
        return None;
    };

    // Don't match inside comments or strings
    if line.starts_with("//") || line.starts_with("/*") || line.starts_with('*') {
        return None;
    }

    let after_fn = &line[fn_idx + 3..];
    let name_end = after_fn.find(|c: char| !c.is_alphanumeric() && c != '_')?;
    let name = after_fn[..name_end].to_string();

    if name.is_empty() {
        return None;
    }

    // Build signature up to the opening brace or where clause
    let sig_end = line.find('{').unwrap_or(line.len());
    let signature = line[..sig_end].trim().to_string();

    Some((name, signature))
}

fn extract_keyword_name(line: &str, keyword: &str) -> Option<String> {
    // Match: [pub] keyword Name
    let pattern = format!("{} ", keyword);
    let idx = line.find(&pattern)?;

    // Make sure keyword is at the right position (after pub or at start)
    if idx > 0 && !line[..idx].trim().is_empty() {
        let before = line[..idx].trim();
        if before != "pub"
            && before != "pub(crate)"
            && before != "pub(super)"
            && before != "export"
            && before != "export abstract"
            && before != "export default"
        {
            return None;
        }
    }

    // Don't match inside comments
    if line.starts_with("//") || line.starts_with("/*") || line.starts_with('*') {
        return None;
    }

    let after = &line[idx + pattern.len()..];
    let name_end = after
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(after.len());
    let name = after[..name_end].to_string();

    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn extract_typescript_symbols(content: &str) -> Vec<SymbolEntry> {
    let mut symbols = Vec::new();

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // export function / function / async function
        if trimmed.contains("function ") && !trimmed.starts_with("//") && !trimmed.starts_with("*")
        {
            let vis = if trimmed.starts_with("export") {
                Some("export".to_string())
            } else {
                None
            };
            if let Some(idx) = trimmed.find("function ") {
                let after = &trimmed[idx + 9..];
                let name_end = after
                    .find(|c: char| !c.is_alphanumeric() && c != '_')
                    .unwrap_or(after.len());
                let name = after[..name_end].to_string();
                if !name.is_empty() {
                    let sig_end = trimmed.find('{').unwrap_or(trimmed.len());
                    symbols.push(SymbolEntry {
                        kind: SymbolKind::Function,
                        name,
                        signature: Some(trimmed[..sig_end].trim().to_string()),
                        line: line_idx + 1,
                        visibility: vis,
                    });
                }
            }
        }

        // class
        if (trimmed.starts_with("class ")
            || trimmed.starts_with("export class ")
            || trimmed.starts_with("abstract class ")
            || trimmed.starts_with("export abstract class "))
            && !trimmed.starts_with("//")
        {
            if let Some(name) = extract_keyword_name(trimmed, "class") {
                symbols.push(SymbolEntry {
                    kind: SymbolKind::Class,
                    name,
                    signature: None,
                    line: line_idx + 1,
                    visibility: if trimmed.starts_with("export") {
                        Some("export".to_string())
                    } else {
                        None
                    },
                });
            }
        }

        // interface
        if (trimmed.starts_with("interface ") || trimmed.starts_with("export interface "))
            && !trimmed.starts_with("//")
        {
            if let Some(name) = extract_keyword_name(trimmed, "interface") {
                symbols.push(SymbolEntry {
                    kind: SymbolKind::Interface,
                    name,
                    signature: None,
                    line: line_idx + 1,
                    visibility: if trimmed.starts_with("export") {
                        Some("export".to_string())
                    } else {
                        None
                    },
                });
            }
        }

        // type
        if (trimmed.starts_with("type ") || trimmed.starts_with("export type "))
            && !trimmed.starts_with("//")
        {
            if let Some(name) = extract_keyword_name(trimmed, "type") {
                symbols.push(SymbolEntry {
                    kind: SymbolKind::Type,
                    name,
                    signature: None,
                    line: line_idx + 1,
                    visibility: if trimmed.starts_with("export") {
                        Some("export".to_string())
                    } else {
                        None
                    },
                });
            }
        }
    }

    symbols
}

fn extract_python_symbols(content: &str) -> Vec<SymbolEntry> {
    let mut symbols = Vec::new();

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // def function_name(
        if let Some(after) = trimmed.strip_prefix("def ") {
            let name_end = after.find('(').unwrap_or(after.len());
            let name = after[..name_end].trim().to_string();
            if !name.is_empty() {
                let sig_end = trimmed.find(':').unwrap_or(trimmed.len());
                symbols.push(SymbolEntry {
                    kind: SymbolKind::Function,
                    name,
                    signature: Some(trimmed[..sig_end].to_string()),
                    line: line_idx + 1,
                    visibility: None,
                });
            }
        }

        // class ClassName
        if let Some(after) = trimmed.strip_prefix("class ") {
            let name_end = after
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(after.len());
            let name = after[..name_end].to_string();
            if !name.is_empty() {
                symbols.push(SymbolEntry {
                    kind: SymbolKind::Class,
                    name,
                    signature: None,
                    line: line_idx + 1,
                    visibility: None,
                });
            }
        }
    }

    symbols
}

fn extract_go_symbols(content: &str) -> Vec<SymbolEntry> {
    let mut symbols = Vec::new();

    for (line_idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // func Name( or func (receiver) Name(
        if let Some(after) = trimmed.strip_prefix("func ") {
            let (name, kind) = if after.starts_with('(') {
                // Method: func (r *Receiver) Name(
                if let Some(close_paren) = after.find(") ") {
                    let rest = &after[close_paren + 2..];
                    let name_end = rest
                        .find(|c: char| !c.is_alphanumeric() && c != '_')
                        .unwrap_or(rest.len());
                    (rest[..name_end].to_string(), SymbolKind::Method)
                } else {
                    continue;
                }
            } else {
                // Function: func Name(
                let name_end = after
                    .find(|c: char| !c.is_alphanumeric() && c != '_')
                    .unwrap_or(after.len());
                (after[..name_end].to_string(), SymbolKind::Function)
            };

            if !name.is_empty() {
                let vis = if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                    Some("pub".to_string())
                } else {
                    None
                };
                let sig_end = trimmed.find('{').unwrap_or(trimmed.len());
                symbols.push(SymbolEntry {
                    kind,
                    name,
                    signature: Some(trimmed[..sig_end].trim().to_string()),
                    line: line_idx + 1,
                    visibility: vis,
                });
            }
        }

        // type Name struct/interface
        if let Some(after) = trimmed.strip_prefix("type ") {
            let name_end = after
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .unwrap_or(after.len());
            let name = after[..name_end].to_string();
            let rest = after[name_end..].trim();
            let kind = if rest.starts_with("struct") {
                SymbolKind::Struct
            } else if rest.starts_with("interface") {
                SymbolKind::Interface
            } else {
                SymbolKind::Type
            };

            if !name.is_empty() {
                let vis = if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                    Some("pub".to_string())
                } else {
                    None
                };
                symbols.push(SymbolEntry {
                    kind,
                    name,
                    signature: None,
                    line: line_idx + 1,
                    visibility: vis,
                });
            }
        }
    }

    symbols
}

fn format_repo_map(entries: &[RepoMapEntry], root: &Path) -> String {
    let mut lines = Vec::new();
    lines.push(format!("Repository map of {}", root.display()));
    lines.push(format!("{} files", entries.len()));
    lines.push(String::new());

    for entry in entries {
        if entry.symbols.is_empty() {
            lines.push(format!("📄 {}", entry.file));
        } else {
            lines.push(format!(
                "📄 {} ({} symbols)",
                entry.file,
                entry.symbols.len()
            ));
            for symbol in &entry.symbols {
                lines.push(format!("  {}", symbol.format_compact()));
            }
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn test_is_source_file() {
        assert!(is_source_file("main.rs"));
        assert!(is_source_file("app.ts"));
        assert!(is_source_file("index.js"));
        assert!(is_source_file("lib.py"));
        assert!(is_source_file("main.go"));
        assert!(!is_source_file("readme.md"));
        assert!(!is_source_file("data.json"));
        assert!(!is_source_file("image.png"));
    }

    #[test]
    fn test_is_ignored_dir() {
        assert!(is_ignored_dir("node_modules"));
        assert!(is_ignored_dir("target"));
        assert!(is_ignored_dir("__pycache__"));
        assert!(is_ignored_dir(".git"));
        assert!(!is_ignored_dir("src"));
        assert!(!is_ignored_dir("lib"));
    }

    #[test]
    fn test_matches_glob() {
        assert!(matches_glob("main.rs", "*.rs"));
        assert!(!matches_glob("main.py", "*.rs"));
        assert!(matches_glob("test_utils.py", "test"));
    }

    #[test]
    fn test_extract_rust_fn() {
        assert_eq!(
            extract_rust_fn("pub fn process(input: &str) -> Result<()> {"),
            Some((
                "process".to_string(),
                "pub fn process(input: &str) -> Result<()>".to_string()
            ))
        );
        assert_eq!(
            extract_rust_fn("fn helper() {"),
            Some(("helper".to_string(), "fn helper()".to_string()))
        );
        assert_eq!(
            extract_rust_fn("pub async fn run() -> anyhow::Result<()> {"),
            Some((
                "run".to_string(),
                "pub async fn run() -> anyhow::Result<()>".to_string()
            ))
        );
        assert_eq!(extract_rust_fn("// fn commented_out() {"), None);
        assert_eq!(extract_rust_fn("let x = 5;"), None);
    }

    #[test]
    fn test_extract_keyword_name() {
        assert_eq!(
            extract_keyword_name("pub struct Config {", "struct"),
            Some("Config".to_string())
        );
        assert_eq!(
            extract_keyword_name("struct Inner {", "struct"),
            Some("Inner".to_string())
        );
        assert_eq!(
            extract_keyword_name("pub enum Status {", "enum"),
            Some("Status".to_string())
        );
        assert_eq!(
            extract_keyword_name("pub trait Handler {", "trait"),
            Some("Handler".to_string())
        );
        assert_eq!(extract_keyword_name("// struct Commented", "struct"), None);
    }

    #[test]
    fn test_extract_rust_symbols() {
        let code = r#"
pub mod config;

pub struct AppConfig {
    pub name: String,
}

pub enum Status {
    Active,
    Inactive,
}

pub trait Handler {
    fn handle(&self);
}

pub fn process(input: &str) -> Result<()> {
    // ...
}

fn helper() {
    // ...
}
"#;
        let symbols = extract_rust_symbols(code);
        assert!(symbols
            .iter()
            .any(|s| s.name == "config" && s.kind == SymbolKind::Module));
        assert!(symbols
            .iter()
            .any(|s| s.name == "AppConfig" && s.kind == SymbolKind::Struct));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Status" && s.kind == SymbolKind::Enum));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Handler" && s.kind == SymbolKind::Trait));
        assert!(symbols
            .iter()
            .any(|s| s.name == "process" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "helper" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_extract_python_symbols() {
        let code = r#"
class MyClass:
    def method(self):
        pass

def standalone_function(x: int) -> str:
    return str(x)
"#;
        let symbols = extract_python_symbols(code);
        assert!(symbols
            .iter()
            .any(|s| s.name == "MyClass" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "method" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "standalone_function" && s.kind == SymbolKind::Function));
    }

    #[test]
    fn test_extract_go_symbols() {
        let code = r#"
func ProcessData(input string) error {
    return nil
}

func (s *Server) handleRequest(w http.ResponseWriter, r *http.Request) {
}

type Config struct {
    Name string
}

type Handler interface {
    Handle() error
}
"#;
        let symbols = extract_go_symbols(code);
        assert!(symbols.iter().any(|s| s.name == "ProcessData"
            && s.kind == SymbolKind::Function
            && s.visibility == Some("pub".to_string())));
        assert!(symbols.iter().any(|s| s.name == "handleRequest"
            && s.kind == SymbolKind::Method
            && s.visibility.is_none()));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Config" && s.kind == SymbolKind::Struct));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Handler" && s.kind == SymbolKind::Interface));
    }

    #[test]
    fn test_extract_typescript_symbols() {
        let code = r#"
export function processData(input: string): void {
}

export class ApiService {
}

export interface Config {
}

export type Status = 'active' | 'inactive';
"#;
        let symbols = extract_typescript_symbols(code);
        assert!(symbols
            .iter()
            .any(|s| s.name == "processData" && s.kind == SymbolKind::Function));
        assert!(symbols
            .iter()
            .any(|s| s.name == "ApiService" && s.kind == SymbolKind::Class));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Config" && s.kind == SymbolKind::Interface));
        assert!(symbols
            .iter()
            .any(|s| s.name == "Status" && s.kind == SymbolKind::Type));
    }

    #[test]
    fn test_generate_repo_map() {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(
            dir.path().join("src/main.rs"),
            "pub fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("src/lib.rs"),
            "pub struct Config {}\npub fn init() {}\n",
        )
        .unwrap();

        let entries = generate_repo_map(dir.path(), 10, 500, None, true).unwrap();
        assert_eq!(entries.len(), 2);

        // Check symbols were extracted
        let total_symbols: usize = entries.iter().map(|e| e.symbols.len()).sum();
        assert!(total_symbols > 0);
    }

    #[test]
    fn test_generate_repo_map_with_include_filter() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(dir.path().join("app.py"), "def main(): pass\n").unwrap();

        let entries = generate_repo_map(dir.path(), 10, 500, Some("*.rs"), true).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].file.ends_with(".rs"));
    }

    #[test]
    fn test_generate_repo_map_max_files() {
        let dir = TempDir::new().unwrap();
        for i in 0..10 {
            fs::write(dir.path().join(format!("file_{}.rs", i)), "fn test() {}\n").unwrap();
        }

        let entries = generate_repo_map(dir.path(), 10, 3, None, false).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_format_repo_map() {
        let entries = vec![RepoMapEntry {
            file: "src/main.rs".to_string(),
            symbols: vec![SymbolEntry {
                kind: SymbolKind::Function,
                name: "main".to_string(),
                signature: Some("pub fn main()".to_string()),
                line: 1,
                visibility: Some("pub".to_string()),
            }],
        }];
        let output = format_repo_map(&entries, Path::new("/test"));
        assert!(output.contains("src/main.rs"));
        assert!(output.contains("pub fn main()"));
    }

    /// The two symbol backends must agree on the symbols that matter.
    ///
    /// The regex path is the fallback when `dev-tools` is off, so a user who disables
    /// the feature should get a smaller map, not a different or wrong one. This runs
    /// only when tree-sitter is compiled in, since it is comparing the two.
    #[cfg(feature = "dev-tools")]
    #[test]
    fn treesitter_and_regex_backends_agree_on_rust_symbols() {
        let src = "\
pub struct Config {}
pub enum Mode {}
pub trait Runner {}
pub fn exported() {}
fn private() {}
";
        let ts = super::super::treesitter::extract_symbols(src, "rs").unwrap();
        let re = extract_symbols_with_regex(src, "rs");

        let mut ts_names: Vec<&str> = ts.iter().map(|s| s.name.as_str()).collect();
        let mut re_names: Vec<&str> = re.iter().map(|s| s.name.as_str()).collect();
        ts_names.sort_unstable();
        re_names.sort_unstable();

        // Every symbol the regex extractor finds must also be found by tree-sitter.
        // The reverse need not hold: tree-sitter understands the grammar and picks up
        // constructs the line-oriented regexes miss.
        for name in &re_names {
            assert!(
                ts_names.contains(name),
                "tree-sitter missed `{name}` that regex found: ts={ts_names:?} regex={re_names:?}"
            );
        }
    }
}
