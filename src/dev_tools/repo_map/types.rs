use serde::{Deserialize, Serialize};

/// Kind of symbol extracted from source code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolKind {
    Module,
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Interface,
    Class,
    Type,
    Constant,
    Import,
}

impl SymbolKind {
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Module => "mod",
            Self::Function => "fn",
            Self::Method => "fn",
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Trait => "trait",
            Self::Interface => "interface",
            Self::Class => "class",
            Self::Type => "type",
            Self::Constant => "const",
            Self::Import => "use",
        }
    }
}

/// An entry in the repo map.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoMapEntry {
    pub file: String,
    pub symbols: Vec<SymbolEntry>,
}

/// A single symbol within a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolEntry {
    pub kind: SymbolKind,
    pub name: String,
    pub signature: Option<String>,
    pub line: usize,
    pub visibility: Option<String>,
}

impl SymbolEntry {
    pub fn format_compact(&self) -> String {
        let vis = self.visibility.as_deref().unwrap_or("");
        let vis_prefix = if vis.is_empty() {
            String::new()
        } else {
            format!("{} ", vis)
        };

        if let Some(ref sig) = self.signature {
            format!("{}{}  (L{})", vis_prefix, sig, self.line)
        } else {
            format!(
                "{}{} {}  (L{})",
                vis_prefix,
                self.kind.icon(),
                self.name,
                self.line
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_symbol_kind_icon() {
        assert_eq!(SymbolKind::Function.icon(), "fn");
        assert_eq!(SymbolKind::Struct.icon(), "struct");
        assert_eq!(SymbolKind::Enum.icon(), "enum");
        assert_eq!(SymbolKind::Module.icon(), "mod");
        assert_eq!(SymbolKind::Trait.icon(), "trait");
    }

    #[test]
    fn test_symbol_entry_format_compact() {
        let entry = SymbolEntry {
            kind: SymbolKind::Function,
            name: "process".to_string(),
            signature: Some("fn process(input: &str) -> Result<()>".to_string()),
            line: 42,
            visibility: Some("pub".to_string()),
        };
        let formatted = entry.format_compact();
        assert!(formatted.contains("pub"));
        assert!(formatted.contains("fn process(input: &str) -> Result<()>"));
        assert!(formatted.contains("L42"));
    }

    #[test]
    fn test_symbol_entry_format_compact_no_sig() {
        let entry = SymbolEntry {
            kind: SymbolKind::Struct,
            name: "Config".to_string(),
            signature: None,
            line: 10,
            visibility: None,
        };
        let formatted = entry.format_compact();
        assert_eq!(formatted, "struct Config  (L10)");
    }
}
