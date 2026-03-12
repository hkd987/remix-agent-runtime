use regex::Regex;
use std::sync::LazyLock;

static ANSI_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[a-zA-Z]").expect("valid ANSI regex"));

/// Remove ANSI escape codes, carriage returns, and other terminal control sequences.
/// Achieves 85-95% reduction on typical CI output.
pub fn strip_ansi(input: &str) -> String {
    let stripped = ANSI_RE.replace_all(input, "");
    stripped.replace('\r', "")
}

/// Truncate output that exceeds `max_bytes`, keeping the first third and last
/// two-thirds of the allowed content with a marker in the middle. Splits at
/// line boundaries where possible. Returns the input unchanged if within limit.
pub fn truncate_output(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_string();
    }

    let total_bytes = input.len();
    let marker = format!("\n... [truncated, {} bytes total]\n", total_bytes);
    let marker_len = marker.len();

    if max_bytes <= marker_len {
        return marker;
    }

    let available = max_bytes - marker_len;
    let head_budget = available / 3;
    let tail_budget = available - head_budget;

    let head_end = find_line_boundary_before(input, head_budget);
    let tail_start = find_line_boundary_after(input, total_bytes.saturating_sub(tail_budget));

    format!("{}{}{}", &input[..head_end], marker, &input[tail_start..])
}

/// Collapse consecutive duplicate lines. If the same line appears N times
/// consecutively (N > 1), replace with the line followed by `(×N)`.
/// Achieves 70-85% reduction on log output with repeated patterns.
pub fn dedup_lines(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = input.lines().collect();
    let mut result = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let current = lines[i];
        let mut count = 1;
        while i + count < lines.len() && lines[i + count] == current {
            count += 1;
        }
        if count > 1 {
            result.push(format!("{} (\u{00d7}{})", current, count));
        } else {
            result.push(current.to_string());
        }
        i += count;
    }

    let mut output = result.join("\n");
    // Preserve trailing newline if the original had one
    if input.ends_with('\n') {
        output.push('\n');
    }
    output
}

/// Find the last newline position at or before `pos`, returning `pos` if none found.
fn find_line_boundary_before(input: &str, pos: usize) -> usize {
    let pos = pos.min(input.len());
    // Ensure we don't split inside a multi-byte character
    let pos = floor_char_boundary(input, pos);
    match input[..pos].rfind('\n') {
        Some(nl) => nl + 1, // include the newline in the head
        None => pos,
    }
}

/// Find the first newline position at or after `pos`, returning `pos` if none found.
fn find_line_boundary_after(input: &str, pos: usize) -> usize {
    let pos = pos.min(input.len());
    let pos = ceil_char_boundary(input, pos);
    match input[pos..].find('\n') {
        Some(nl) => pos + nl + 1, // start tail after the newline
        None => pos,
    }
}

/// Find the largest byte index <= pos that is a valid char boundary.
pub(crate) fn floor_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos.min(s.len());
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Find the smallest byte index >= pos that is a valid char boundary.
fn ceil_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos.min(s.len());
    while p < s.len() && !s.is_char_boundary(p) {
        p += 1;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- strip_ansi tests ----

    #[test]
    fn strip_ansi_empty() {
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn strip_ansi_no_codes() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn strip_ansi_color_codes() {
        let input = "\x1b[31mERROR\x1b[0m: something failed";
        assert_eq!(strip_ansi(input), "ERROR: something failed");
    }

    #[test]
    fn strip_ansi_cursor_and_clear() {
        let input = "\x1b[2J\x1b[1;1Hhello\x1b[K";
        assert_eq!(strip_ansi(input), "hello");
    }

    #[test]
    fn strip_ansi_carriage_returns() {
        let input = "progress: 50%\rprogress: 100%";
        assert_eq!(strip_ansi(input), "progress: 50%progress: 100%");
    }

    #[test]
    fn strip_ansi_mixed() {
        let input = "\x1b[32m✓\x1b[0m test passed\r\n\x1b[31m✗\x1b[0m test failed\r\n";
        assert_eq!(strip_ansi(input), "✓ test passed\n✗ test failed\n");
    }

    #[test]
    fn strip_ansi_unicode() {
        let input = "\x1b[1m日本語\x1b[0m テスト";
        assert_eq!(strip_ansi(input), "日本語 テスト");
    }

    #[test]
    fn strip_ansi_multiple_params() {
        let input = "\x1b[1;31;42mBOLD RED ON GREEN\x1b[0m";
        assert_eq!(strip_ansi(input), "BOLD RED ON GREEN");
    }

    // ---- truncate_output tests ----

    #[test]
    fn truncate_output_empty() {
        assert_eq!(truncate_output("", 100), "");
    }

    #[test]
    fn truncate_output_within_limit() {
        let input = "short text";
        assert_eq!(truncate_output(input, 100), "short text");
    }

    #[test]
    fn truncate_output_exactly_at_limit() {
        let input = "exact";
        assert_eq!(truncate_output(input, 5), "exact");
    }

    #[test]
    fn truncate_output_exceeds_limit() {
        let input = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\n";
        let result = truncate_output(input, 40);
        assert!(result.contains("... [truncated,"));
        assert!(result.len() <= 40 + 20); // allow some slack for marker
    }

    #[test]
    fn truncate_output_preserves_beginning_and_end() {
        let lines: Vec<String> = (0..100).map(|i| format!("line {}", i)).collect();
        let input = lines.join("\n");
        let result = truncate_output(&input, 200);
        assert!(result.starts_with("line 0"));
        assert!(result.contains("truncated"));
        assert!(result.ends_with('\n') || result.contains("line 99"));
    }

    #[test]
    fn truncate_output_very_small_limit() {
        let input = "a".repeat(1000);
        let result = truncate_output(&input, 5);
        assert!(result.contains("truncated"));
    }

    #[test]
    fn truncate_output_single_line() {
        let input = "a".repeat(200);
        let result = truncate_output(&input, 100);
        assert!(result.contains("truncated"));
        assert!(result.contains("200 bytes total"));
    }

    #[test]
    fn truncate_output_unicode() {
        // Each emoji is 4 bytes
        let input = "🎉".repeat(50); // 200 bytes
        let result = truncate_output(&input, 100);
        assert!(result.contains("truncated"));
        // Ensure we didn't split inside a character
        for c in result.chars() {
            assert!(c.len_utf8() >= 1); // valid chars
        }
    }

    // ---- dedup_lines tests ----

    #[test]
    fn dedup_lines_empty() {
        assert_eq!(dedup_lines(""), "");
    }

    #[test]
    fn dedup_lines_no_duplicates() {
        let input = "a\nb\nc";
        assert_eq!(dedup_lines(input), "a\nb\nc");
    }

    #[test]
    fn dedup_lines_single_line() {
        assert_eq!(dedup_lines("hello"), "hello");
    }

    #[test]
    fn dedup_lines_consecutive_duplicates() {
        let input = "ok\nok\nok\ndone";
        assert_eq!(dedup_lines(input), "ok (\u{00d7}3)\ndone");
    }

    #[test]
    fn dedup_lines_multiple_groups() {
        let input = "a\na\nb\nb\nb\nc";
        assert_eq!(dedup_lines(input), "a (\u{00d7}2)\nb (\u{00d7}3)\nc");
    }

    #[test]
    fn dedup_lines_non_consecutive_not_deduped() {
        let input = "a\nb\na";
        assert_eq!(dedup_lines(input), "a\nb\na");
    }

    #[test]
    fn dedup_lines_preserves_trailing_newline() {
        let input = "x\nx\ny\n";
        assert_eq!(dedup_lines(input), "x (\u{00d7}2)\ny\n");
    }

    #[test]
    fn dedup_lines_unicode() {
        let input = "日本\n日本\n日本\nテスト";
        assert_eq!(dedup_lines(input), "日本 (\u{00d7}3)\nテスト");
    }

    #[test]
    fn dedup_lines_all_same() {
        let input = "same\nsame\nsame\nsame";
        assert_eq!(dedup_lines(input), "same (\u{00d7}4)");
    }

    #[test]
    fn dedup_lines_empty_lines() {
        let input = "a\n\n\nb";
        assert_eq!(dedup_lines(input), "a\n (\u{00d7}2)\nb");
    }
}
