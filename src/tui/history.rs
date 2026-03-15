use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::PathBuf;

/// Per-working-directory input history with navigation and reverse search.
pub struct InputHistory {
    entries: VecDeque<String>,
    /// Current position when navigating (None = new input).
    position: Option<usize>,
    max_entries: usize,
    file_path: PathBuf,
    search_query: Option<String>,
    search_results: Vec<usize>,
    search_position: usize,
}

impl InputHistory {
    /// Create a new empty history with the given capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_entries),
            position: None,
            max_entries,
            file_path: Self::history_dir().join("default.jsonl"),
            search_query: None,
            search_results: Vec::new(),
            search_position: 0,
        }
    }

    /// Load history for the current working directory.
    pub fn load_for_cwd() -> Result<Self, std::io::Error> {
        let hash = Self::cwd_hash();
        let dir = Self::history_dir();
        std::fs::create_dir_all(&dir)?;
        let file_path = dir.join(format!("{hash}.jsonl"));

        let mut history = Self {
            entries: VecDeque::with_capacity(1000),
            position: None,
            max_entries: 1000,
            file_path: file_path.clone(),
            search_query: None,
            search_results: Vec::new(),
            search_position: 0,
        };

        if file_path.exists() {
            let file = std::fs::File::open(&file_path)?;
            let reader = std::io::BufReader::new(file);
            for line in reader.lines() {
                let line = line?;
                if let Ok(entry) = serde_json::from_str::<String>(&line) {
                    if history.entries.len() >= history.max_entries {
                        history.entries.pop_front();
                    }
                    history.entries.push_back(entry);
                }
            }
        }

        Ok(history)
    }

    /// Add an entry to the history, evicting the oldest if at capacity.
    pub fn add(&mut self, entry: String) {
        if entry.is_empty() {
            return;
        }
        // Remove duplicate if it exists to avoid repeated entries
        if let Some(pos) = self.entries.iter().rposition(|e| e == &entry) {
            self.entries.remove(pos);
        }
        if self.entries.len() >= self.max_entries {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
        self.position = None;
    }

    /// Navigate up (backward) through history.
    /// On first call, saves the current input and goes to the most recent entry.
    pub fn navigate_up(&mut self, _current_input: &str) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        let idx = match self.position {
            None => self.entries.len().checked_sub(1)?,
            Some(i) => {
                if i == 0 {
                    return Some(&self.entries[0]);
                }
                i - 1
            }
        };
        self.position = Some(idx);
        Some(&self.entries[idx])
    }

    /// Navigate down (forward) through history.
    /// Returns None when past the newest entry (back to fresh input).
    pub fn navigate_down(&mut self) -> Option<&str> {
        match self.position {
            None => None,
            Some(i) => {
                if i + 1 >= self.entries.len() {
                    self.position = None;
                    None
                } else {
                    self.position = Some(i + 1);
                    Some(&self.entries[i + 1])
                }
            }
        }
    }

    /// Reset navigation position to "new input".
    pub fn reset_position(&mut self) {
        self.position = None;
    }

    /// Start a reverse search with the given query.
    pub fn start_search(&mut self, query: &str) {
        self.search_query = Some(query.to_string());
        self.search_results = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.contains(query))
            .map(|(i, _)| i)
            .collect();
        // Start from the most recent match
        self.search_position = self.search_results.len().saturating_sub(1);
    }

    /// Get the next (older) search result.
    pub fn search_next(&mut self) -> Option<&str> {
        if self.search_results.is_empty() {
            return None;
        }
        if self.search_position == 0 {
            // Wrap around to the end
            self.search_position = self.search_results.len() - 1;
        } else {
            self.search_position -= 1;
        }
        let idx = self.search_results[self.search_position];
        Some(&self.entries[idx])
    }

    /// Get the previous (newer) search result.
    pub fn search_prev(&mut self) -> Option<&str> {
        if self.search_results.is_empty() {
            return None;
        }
        self.search_position = (self.search_position + 1) % self.search_results.len();
        let idx = self.search_results[self.search_position];
        Some(&self.entries[idx])
    }

    /// End the current search session.
    pub fn end_search(&mut self) {
        self.search_query = None;
        self.search_results.clear();
        self.search_position = 0;
    }

    /// Persist history to disk as JSONL.
    pub fn save(&self) -> Result<(), std::io::Error> {
        if let Some(parent) = self.file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&self.file_path)?;
        for entry in &self.entries {
            let json = serde_json::to_string(entry)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
            writeln!(file, "{json}")?;
        }
        Ok(())
    }

    /// Directory where per-cwd history files are stored.
    fn history_dir() -> PathBuf {
        dirs_or_default().join("chat_history")
    }

    /// Deterministic hex hash of the current working directory.
    pub fn cwd_hash() -> String {
        let cwd = std::env::current_dir().unwrap_or_default();
        simple_hash(cwd.to_string_lossy().as_bytes())
    }
}

/// Returns ~/.remix or a fallback.
fn dirs_or_default() -> PathBuf {
    if let Some(home) = home_dir() {
        home.join(".remix")
    } else {
        PathBuf::from("/tmp/.remix")
    }
}

/// Cross-platform home directory.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Simple deterministic hash producing a hex string.
/// Uses FNV-1a for speed (no crypto needed).
fn simple_hash(data: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;

    #[test]
    fn test_add_and_retrieve() {
        let mut h = InputHistory::new(10);
        h.add("hello".to_string());
        h.add("world".to_string());
        assert_eq!(h.entries.len(), 2);
        assert_eq!(h.entries[0], "hello");
        assert_eq!(h.entries[1], "world");
    }

    #[test]
    fn test_add_empty_is_noop() {
        let mut h = InputHistory::new(10);
        h.add("".to_string());
        assert!(h.entries.is_empty());
    }

    #[test]
    fn test_add_deduplicates() {
        let mut h = InputHistory::new(10);
        h.add("a".to_string());
        h.add("b".to_string());
        h.add("a".to_string());
        assert_eq!(h.entries.len(), 2);
        assert_eq!(h.entries[0], "b");
        assert_eq!(h.entries[1], "a");
    }

    #[test]
    fn test_navigate_up_down() {
        let mut h = InputHistory::new(10);
        h.add("first".to_string());
        h.add("second".to_string());
        h.add("third".to_string());

        assert_eq!(h.navigate_up(""), Some("third"));
        assert_eq!(h.navigate_up(""), Some("second"));
        assert_eq!(h.navigate_up(""), Some("first"));
        // At beginning, stays at first
        assert_eq!(h.navigate_up(""), Some("first"));

        assert_eq!(h.navigate_down(), Some("second"));
        assert_eq!(h.navigate_down(), Some("third"));
        // Past end returns None
        assert_eq!(h.navigate_down(), None);
    }

    #[test]
    fn test_navigate_empty_history() {
        let mut h = InputHistory::new(10);
        assert_eq!(h.navigate_up(""), None);
        assert_eq!(h.navigate_down(), None);
    }

    #[test]
    fn test_fifo_eviction() {
        let mut h = InputHistory::new(3);
        h.add("a".to_string());
        h.add("b".to_string());
        h.add("c".to_string());
        h.add("d".to_string());
        assert_eq!(h.entries.len(), 3);
        assert_eq!(h.entries[0], "b");
        assert_eq!(h.entries[1], "c");
        assert_eq!(h.entries[2], "d");
    }

    #[test]
    fn test_reset_position() {
        let mut h = InputHistory::new(10);
        h.add("a".to_string());
        h.navigate_up("");
        assert!(h.position.is_some());
        h.reset_position();
        assert!(h.position.is_none());
    }

    #[test]
    fn test_reverse_search() {
        let mut h = InputHistory::new(10);
        h.add("cargo build".to_string());
        h.add("git status".to_string());
        h.add("cargo test".to_string());
        h.add("ls -la".to_string());

        h.start_search("cargo");
        assert_eq!(h.search_results.len(), 2);

        // search_next goes to older match
        let result = h.search_next();
        assert_eq!(result, Some("cargo build"));

        // search_prev goes to newer match
        let result = h.search_prev();
        assert_eq!(result, Some("cargo test"));
    }

    #[test]
    fn test_search_no_results() {
        let mut h = InputHistory::new(10);
        h.add("hello".to_string());
        h.start_search("xyz");
        assert!(h.search_results.is_empty());
        assert_eq!(h.search_next(), None);
        assert_eq!(h.search_prev(), None);
    }

    #[test]
    fn test_end_search() {
        let mut h = InputHistory::new(10);
        h.add("hello".to_string());
        h.start_search("hello");
        assert!(h.search_query.is_some());
        h.end_search();
        assert!(h.search_query.is_none());
        assert!(h.search_results.is_empty());
    }

    #[test]
    fn test_save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test_history.jsonl");

        let mut h = InputHistory::new(10);
        h.file_path = file_path.clone();
        h.add("first command".to_string());
        h.add("second command".to_string());
        h.add("third command".to_string());
        h.save().unwrap();

        // Load from the saved file
        let mut loaded = InputHistory::new(10);
        loaded.file_path = file_path.clone();

        let file = std::fs::File::open(&file_path).unwrap();
        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            let line = line.unwrap();
            if let Ok(entry) = serde_json::from_str::<String>(&line) {
                loaded.entries.push_back(entry);
            }
        }

        assert_eq!(loaded.entries.len(), 3);
        assert_eq!(loaded.entries[0], "first command");
        assert_eq!(loaded.entries[1], "second command");
        assert_eq!(loaded.entries[2], "third command");
    }

    #[test]
    fn test_save_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("nested").join("deep").join("history.jsonl");

        let mut h = InputHistory::new(10);
        h.file_path = file_path;
        h.add("test".to_string());
        h.save().unwrap();
    }

    #[test]
    fn test_cwd_hash_deterministic() {
        let h1 = InputHistory::cwd_hash();
        let h2 = InputHistory::cwd_hash();
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 16); // 64-bit hex
    }

    #[test]
    fn test_simple_hash_deterministic() {
        let a = simple_hash(b"hello");
        let b = simple_hash(b"hello");
        assert_eq!(a, b);
    }

    #[test]
    fn test_simple_hash_different_inputs() {
        let a = simple_hash(b"/home/user/project-a");
        let b = simple_hash(b"/home/user/project-b");
        assert_ne!(a, b);
    }

    #[test]
    fn test_load_handles_malformed_lines() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("bad.jsonl");
        {
            let mut f = std::fs::File::create(&file_path).unwrap();
            writeln!(f, "\"good line\"").unwrap();
            writeln!(f, "not valid json").unwrap();
            writeln!(f, "\"another good\"").unwrap();
        }

        let mut h = InputHistory::new(10);
        h.file_path = file_path.clone();

        let file = std::fs::File::open(&file_path).unwrap();
        let reader = std::io::BufReader::new(file);
        for line in reader.lines() {
            let line = line.unwrap();
            if let Ok(entry) = serde_json::from_str::<String>(&line) {
                h.entries.push_back(entry);
            }
        }

        assert_eq!(h.entries.len(), 2);
        assert_eq!(h.entries[0], "good line");
        assert_eq!(h.entries[1], "another good");
    }

    #[test]
    fn test_navigate_single_entry() {
        let mut h = InputHistory::new(10);
        h.add("only".to_string());

        assert_eq!(h.navigate_up(""), Some("only"));
        assert_eq!(h.navigate_up(""), Some("only")); // stays at 0
        assert_eq!(h.navigate_down(), None); // past end
    }
}
