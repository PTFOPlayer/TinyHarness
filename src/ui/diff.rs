use std::{error::Error, io::Write};

use crate::style::*;

// ── Diff computation (LCS-based algorithm) ─────────────────────────────────

/// A single line in a diff hunk — either kept, removed, or added.
#[derive(Debug, Clone, PartialEq)]
pub enum DiffLine<'a> {
    /// Line present in both old and new (unchanged).
    Keep(&'a str),
    /// Line present only in old (removed).
    Remove(&'a str),
    /// Line present only in new (added).
    Add(&'a str),
}

/// Compute the shortest edit script between `old` and `new`.
///
/// Uses a standard LCS (Longest Common Subsequence) dynamic programming approach
/// to find the optimal alignment, then walks the DP table to produce the diff.
/// This produces the same quality of diff as `git diff` for line-level changes.
pub fn compute_diff<'a>(old: &'a [&str], new: &'a [&str]) -> Vec<DiffLine<'a>> {
    let n = old.len();
    let m = new.len();

    // Trivial cases
    if n == 0 && m == 0 {
        return vec![];
    }
    if n == 0 {
        return new.iter().map(|l| DiffLine::Add(l)).collect();
    }
    if m == 0 {
        return old.iter().map(|l| DiffLine::Remove(l)).collect();
    }
    if old == new {
        return old.iter().map(|l| DiffLine::Keep(l)).collect();
    }

    // Build LCS length table using O(n) space (rolling rows).
    // dp[j] = LCS length for old[0..i] and new[0..j]
    let mut dp = vec![0usize; m + 1];
    let mut prev = vec![0usize; m + 1];

    // We also need to store the full table for backtracking.
    // For correctness we store the full (n+1) x (m+1) table.
    // This is fine since diffs are typically small (file previews in confirmation prompts).
    let mut table = vec![vec![0usize; m + 1]; n + 1];

    for i in 1..=n {
        prev.copy_from_slice(&dp);
        for j in 1..=m {
            if old[i - 1] == new[j - 1] {
                dp[j] = prev[j - 1] + 1;
            } else {
                dp[j] = dp[j - 1].max(prev[j]);
            }
        }
        table[i].copy_from_slice(&dp);
    }

    // Backtrack through the full table to reconstruct the diff
    let mut result = Vec::with_capacity(n + m);
    let mut i = n;
    let mut j = m;

    while i > 0 || j > 0 {
        if i > 0 && j > 0 && old[i - 1] == new[j - 1] {
            result.push(DiffLine::Keep(old[i - 1]));
            i -= 1;
            j -= 1;
        } else if j > 0 && (i == 0 || table[i][j - 1] >= table[i - 1][j]) {
            result.push(DiffLine::Add(new[j - 1]));
            j -= 1;
        } else {
            result.push(DiffLine::Remove(old[i - 1]));
            i -= 1;
        }
    }

    result.reverse();
    result
}

/// Context lines to show around each change in unified diff output.
const DIFF_CONTEXT_LINES: usize = 3;

/// Render a [`DiffLine`] sequence into unified-diff-style output with line
/// numbers, `+`/`-`/` ` prefixes, and configurable context around changes.
#[allow(unused_assignments)]
fn render_diff_lines<W: Write>(
    stdout: &mut W,
    diff: &[DiffLine],
    old_lines: &[&str],
    new_lines: &[&str],
    show_line_numbers: bool,
) -> Result<(), Box<dyn Error>> {
    if diff.is_empty() {
        return Ok(());
    }

    // Compute line number width
    let max_line = old_lines.len().max(new_lines.len()).max(1);
    let num_width = if show_line_numbers {
        max_line.to_string().len().max(2)
    } else {
        0
    };

    // Find hunks: groups of changes with context around them.
    let change_indices: Vec<usize> = diff
        .iter()
        .enumerate()
        .filter_map(|(i, l)| matches!(l, DiffLine::Remove(_) | DiffLine::Add(_)).then_some(i))
        .collect();

    if change_indices.is_empty() {
        return Ok(());
    }

    // Merge overlapping/adjacent hunks
    let mut hunk_ranges: Vec<(usize, usize)> = Vec::new();
    let mut hunk_start = change_indices[0].saturating_sub(DIFF_CONTEXT_LINES);
    let mut hunk_end = (change_indices[0] + DIFF_CONTEXT_LINES + 1).min(diff.len());

    for &idx in &change_indices[1..] {
        let new_start = idx.saturating_sub(DIFF_CONTEXT_LINES);
        let new_end = (idx + DIFF_CONTEXT_LINES + 1).min(diff.len());

        if new_start <= hunk_end {
            hunk_end = hunk_end.max(new_end);
        } else {
            hunk_ranges.push((hunk_start, hunk_end));
            hunk_start = new_start;
            hunk_end = new_end;
        }
    }
    hunk_ranges.push((hunk_start, hunk_end));

    // Render each hunk
    for (hunk_idx, &(start, end)) in hunk_ranges.iter().enumerate() {
        // Add separator between hunks
        if hunk_idx > 0 {
            let sep_line = format!(
                "{}  {}{}",
                DIM,
                BOX_COLOR,
                "·".repeat(if show_line_numbers {
                    num_width + 1 + 3 + 60
                } else {
                    3 + 60
                })
            );
            writeln!(stdout, "{}{}", sep_line, RESET)?;
        }

        // Count line numbers up to the start of this hunk
        let mut old_num: usize = 0;
        #[allow(unused_variables)]
        let mut new_num: usize = 0;
        for item in diff.iter().take(start) {
            match item {
                DiffLine::Keep(_) => {
                    old_num += 1;
                    new_num += 1;
                }
                DiffLine::Remove(_) => {
                    old_num += 1;
                }
                DiffLine::Add(_) => {
                    new_num += 1;
                }
            }
        }

        for i in start..end {
            if i >= diff.len() {
                break;
            }
            match &diff[i] {
                DiffLine::Keep(line) => {
                    old_num += 1;
                    new_num += 1;
                    if show_line_numbers {
                        writeln!(
                            stdout,
                            "  {:>width$} {} {}",
                            old_num,
                            DIM,
                            line,
                            width = num_width,
                        )?;
                    } else {
                        writeln!(stdout, "  {} {}{}", DIM, line, RESET)?;
                    }
                }
                DiffLine::Remove(line) => {
                    old_num += 1;
                    if show_line_numbers {
                        writeln!(
                            stdout,
                            "{}  {:<width$} {}{} {}",
                            RED,
                            "-",
                            RESET,
                            RED,
                            line,
                            width = num_width
                        )?;
                    } else {
                        writeln!(stdout, "{}  - {}{}", RED, RESET, line)?;
                    }
                }
                DiffLine::Add(line) => {
                    new_num += 1;
                    if show_line_numbers {
                        writeln!(
                            stdout,
                            "{}  {:<width$} {}{} {}",
                            GREEN,
                            "+",
                            RESET,
                            GREEN,
                            line,
                            width = num_width
                        )?;
                    } else {
                        writeln!(stdout, "{}  + {}{}", GREEN, RESET, line)?;
                    }
                }
            }
        }
    }

    Ok(())
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Show a unified-diff-style view of a write operation (full file).
/// If the file already exists, reads it and shows a line-by-line diff
/// with removed lines in red and added lines in green.
/// If the file is new, shows a preview of the content that will be written.
pub fn show_write_preview<W: Write>(
    stdout: &mut W,
    path: &str,
    new_content: &str,
) -> Result<(), Box<dyn Error>> {
    let existing = std::fs::read_to_string(path);

    let old_lines: Vec<&str> = match &existing {
        Ok(content) => content.lines().collect(),
        Err(_) => {
            // File doesn't exist — show a green preview
            writeln!(
                stdout,
                "\n{}  ── {}New file: {}{} ──{}",
                BOLD, BOX_COLOR, path, BOLD, RESET
            )?;
            writeln!(
                stdout,
                "{}     {}{} {}",
                DIM,
                BOX_COLOR,
                "┄".repeat(60),
                RESET
            )?;
            for line in new_content.lines() {
                writeln!(stdout, "{}  + {}{}", GREEN, RESET, line)?;
            }
            writeln!(stdout, "{}     {} {}", DIM, "┄".repeat(60), RESET)?;
            return Ok(());
        }
    };

    let new_lines: Vec<&str> = new_content.lines().collect();

    // Compute the diff
    let diff = compute_diff(&old_lines, &new_lines);

    // If the file is identical, show a note
    let has_changes = diff.iter().any(|l| !matches!(l, DiffLine::Keep(_)));

    if !has_changes {
        writeln!(
            stdout,
            "\n{}  ── {}No changes in {}{} ──{}",
            BOLD, BOX_COLOR, path, BOLD, RESET
        )?;
        return Ok(());
    }

    // Count additions and removals for the header
    let removals = diff
        .iter()
        .filter(|l| matches!(l, DiffLine::Remove(_)))
        .count();
    let additions = diff
        .iter()
        .filter(|l| matches!(l, DiffLine::Add(_)))
        .count();

    writeln!(
        stdout,
        "\n{}  ── {}Diff for {}{} ── {}{}-{}{}+{}{}",
        BOLD, BOX_COLOR, path, BOLD, RED, removals, GREEN, additions, RESET, RESET,
    )?;

    let max_line = old_lines.len().max(new_lines.len()).max(1);
    let num_width = max_line.to_string().len().max(2);

    // Separator line
    writeln!(
        stdout,
        "{}  {}{} {} {}",
        DIM,
        BOX_COLOR,
        " ".repeat(num_width),
        "┄".repeat(60),
        RESET
    )?;

    // Render the diff with line numbers
    render_diff_lines(stdout, &diff, &old_lines, &new_lines, true)?;

    // Ending separator
    writeln!(
        stdout,
        "{}  {}{} {} {}",
        DIM,
        BOX_COLOR,
        " ".repeat(num_width),
        "┄".repeat(60),
        RESET
    )?;

    Ok(())
}

/// Show a unified-diff-style view of an edit operation.
/// Reads the file, locates `old_str`, and prints context lines
/// with the removed text in red and the replacement in green.
pub fn show_edit_diff<W: Write>(
    stdout: &mut W,
    path: &str,
    old_str: &str,
    new_str: &str,
) -> Result<(), Box<dyn Error>> {
    let content =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read '{}': {}", path, e))?;

    // Find the byte offset of old_str in the content
    let offset = match content.find(old_str) {
        Some(o) => o,
        None => {
            writeln!(
                stdout,
                "  {}[diff error: 'old_str' not found in file]{}",
                RED, RESET
            )?;
            return Ok(());
        }
    };

    // Count newlines before the match to get the line number (0-based)
    let line_number = content[..offset].matches('\n').count();
    let lines: Vec<&str> = content.lines().collect();

    // Split old_str and new_str into lines
    let old_lines: Vec<&str> = old_str.lines().collect();
    let new_lines: Vec<&str> = new_str.lines().collect();

    // Determine context window (show up to 2 lines before and after)
    let before_ctx = 2usize;
    let after_ctx = 2usize;
    let start_line = line_number.saturating_sub(before_ctx);
    let end_line = (line_number + old_lines.len() + after_ctx).min(lines.len());
    let line_num_width = (end_line + 1).to_string().len().max(2);

    writeln!(
        stdout,
        "\n{}  ── {}Diff for {}{} ──{}",
        BOLD, BOX_COLOR, path, BOLD, RESET
    )?;

    // Separator line
    writeln!(
        stdout,
        "{}  {}{} {} {}",
        DIM,
        BOX_COLOR,
        " ".repeat(line_num_width),
        "┄".repeat(60),
        RESET
    )?;

    // Lines before the change
    for (i, line) in lines.iter().enumerate().take(line_number).skip(start_line) {
        writeln!(
            stdout,
            "  {:>width$} {} {}",
            i + 1,
            DIM,
            line,
            width = line_num_width,
        )?;
    }

    // Removed lines (old_str) – shown in red with '-'
    for line in old_lines.iter() {
        writeln!(
            stdout,
            "{}  {:<width$} {}{} {}",
            RED,
            "-",
            RESET,
            RED,
            line,
            width = line_num_width
        )?;
    }

    // Added lines (new_str) – shown in green with '+'
    for line in new_lines.iter() {
        writeln!(
            stdout,
            "{}  {:<width$} {}{} {}",
            GREEN,
            "+",
            RESET,
            GREEN,
            line,
            width = line_num_width
        )?;
    }

    // Lines after the change
    let after_start = line_number + old_lines.len();
    for i in after_start..end_line {
        if i < lines.len() {
            writeln!(
                stdout,
                "  {:>width$} {} {}",
                i + 1,
                DIM,
                lines[i],
                width = line_num_width,
            )?;
        }
    }

    // Ending separator
    writeln!(
        stdout,
        "{}  {}{} {} {}",
        DIM,
        BOX_COLOR,
        " ".repeat(line_num_width),
        "┄".repeat(60),
        RESET
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_diff_identical() {
        let old = vec!["a", "b", "c"];
        let new = vec!["a", "b", "c"];
        let diff = compute_diff(&old, &new);
        assert_eq!(
            diff,
            vec![
                DiffLine::Keep("a"),
                DiffLine::Keep("b"),
                DiffLine::Keep("c"),
            ]
        );
    }

    #[test]
    fn test_compute_diff_empty_to_lines() {
        let old: Vec<&str> = vec![];
        let new = vec!["a", "b"];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff, vec![DiffLine::Add("a"), DiffLine::Add("b")]);
    }

    #[test]
    fn test_compute_diff_lines_to_empty() {
        let old = vec!["a", "b"];
        let new: Vec<&str> = vec![];
        let diff = compute_diff(&old, &new);
        assert_eq!(diff, vec![DiffLine::Remove("a"), DiffLine::Remove("b")]);
    }

    #[test]
    fn test_compute_diff_simple_insertion() {
        let old = vec!["a", "c"];
        let new = vec!["a", "b", "c"];
        let diff = compute_diff(&old, &new);
        assert_eq!(
            diff,
            vec![DiffLine::Keep("a"), DiffLine::Add("b"), DiffLine::Keep("c"),]
        );
    }

    #[test]
    fn test_compute_diff_simple_deletion() {
        let old = vec!["a", "b", "c"];
        let new = vec!["a", "c"];
        let diff = compute_diff(&old, &new);
        assert_eq!(
            diff,
            vec![
                DiffLine::Keep("a"),
                DiffLine::Remove("b"),
                DiffLine::Keep("c"),
            ]
        );
    }

    #[test]
    fn test_compute_diff_replace() {
        let old = vec!["a", "old", "c"];
        let new = vec!["a", "new", "c"];
        let diff = compute_diff(&old, &new);
        assert_eq!(
            diff,
            vec![
                DiffLine::Keep("a"),
                DiffLine::Remove("old"),
                DiffLine::Add("new"),
                DiffLine::Keep("c"),
            ]
        );
    }

    #[test]
    fn test_compute_diff_multiple_changes() {
        let old = vec!["a", "b", "c", "d", "e"];
        let new = vec!["a", "x", "c", "y", "e"];
        let diff = compute_diff(&old, &new);
        assert_eq!(
            diff,
            vec![
                DiffLine::Keep("a"),
                DiffLine::Remove("b"),
                DiffLine::Add("x"),
                DiffLine::Keep("c"),
                DiffLine::Remove("d"),
                DiffLine::Add("y"),
                DiffLine::Keep("e"),
            ]
        );
    }

    #[test]
    fn test_compute_diff_large_shift() {
        // Lines rearranged: the old "middle" section is removed and new lines added
        let old = vec!["keep1", "remove1", "remove2", "remove3", "keep2"];
        let new = vec!["keep1", "add1", "add2", "keep2"];
        let diff = compute_diff(&old, &new);
        let keeps: Vec<_> = diff
            .iter()
            .filter(|l| matches!(l, DiffLine::Keep(_)))
            .collect();
        let removes: Vec<_> = diff
            .iter()
            .filter(|l| matches!(l, DiffLine::Remove(_)))
            .collect();
        let adds: Vec<_> = diff
            .iter()
            .filter(|l| matches!(l, DiffLine::Add(_)))
            .collect();
        assert_eq!(keeps.len(), 2);
        assert_eq!(removes.len(), 3);
        assert_eq!(adds.len(), 2);
        // Keep lines should be in order
        assert!(matches!(keeps[0], DiffLine::Keep("keep1")));
        assert!(matches!(keeps[1], DiffLine::Keep("keep2")));
    }

    #[test]
    fn test_compute_diff_both_empty() {
        let old: Vec<&str> = vec![];
        let new: Vec<&str> = vec![];
        let diff = compute_diff(&old, &new);
        assert!(diff.is_empty());
    }

    #[test]
    fn test_compute_diff_single_line_change() {
        let old = vec!["hello world"];
        let new = vec!["hello rust"];
        let diff = compute_diff(&old, &new);
        assert_eq!(
            diff,
            vec![DiffLine::Remove("hello world"), DiffLine::Add("hello rust")]
        );
    }

    #[test]
    fn test_compute_diff_prepend_and_append() {
        let old = vec!["b", "c"];
        let new = vec!["a", "b", "c", "d"];
        let diff = compute_diff(&old, &new);
        assert_eq!(
            diff,
            vec![
                DiffLine::Add("a"),
                DiffLine::Keep("b"),
                DiffLine::Keep("c"),
                DiffLine::Add("d"),
            ]
        );
    }

    #[test]
    fn test_compute_diff_minimality() {
        // The diff should produce the minimum number of edits
        let old = vec!["a", "b", "c", "d", "e", "f"];
        let new = vec!["a", "c", "d", "e", "g"];
        let diff = compute_diff(&old, &new);
        let removes = diff
            .iter()
            .filter(|l| matches!(l, DiffLine::Remove(_)))
            .count();
        let adds = diff
            .iter()
            .filter(|l| matches!(l, DiffLine::Add(_)))
            .count();
        // Minimum: remove b, remove f, add g = 2 removes + 1 add = 3 edits
        assert_eq!(removes, 2);
        assert_eq!(adds, 1);
    }

    #[test]
    fn test_compute_diff_longer_file() {
        // A more realistic scenario with many lines
        let old: Vec<String> = (0..50).map(|i| format!("line {}", i)).collect();
        let mut new = old.clone();
        // Change lines 10, 20, remove line 30, insert after line 40
        new[10] = "line 10 modified".to_string();
        new[20] = "line 20 modified".to_string();
        new.remove(30);
        new.insert(41, "inserted line".to_string());

        let old_refs: Vec<&str> = old.iter().map(|s| s.as_str()).collect();
        let new_refs: Vec<&str> = new.iter().map(|s| s.as_str()).collect();

        let diff = compute_diff(&old_refs, &new_refs);

        let removes = diff
            .iter()
            .filter(|l| matches!(l, DiffLine::Remove(_)))
            .count();
        let adds = diff
            .iter()
            .filter(|l| matches!(l, DiffLine::Add(_)))
            .count();
        // 3 removals (line 10 original, line 20 original, line 30) and 3 additions
        // (modified 10, modified 20, inserted)
        assert_eq!(removes, 3);
        assert_eq!(adds, 3);
    }

    #[test]
    fn test_compute_diff_reorder() {
        // Swapping two unique lines
        let old = vec!["alpha", "beta"];
        let new = vec!["beta", "alpha"];
        let diff = compute_diff(&old, &new);
        // Both lines exist in both files, so one should be kept.
        // The minimal diff removes and re-adds the reordered line.
        let keeps = diff
            .iter()
            .filter(|l| matches!(l, DiffLine::Keep(_)))
            .count();
        assert!(keeps >= 1, "At least one line should be kept in a reorder");
    }

    #[test]
    fn test_compute_diff_all_same_then_change() {
        // Large common prefix, then a change
        let old: Vec<&str> = vec!["line1", "line2", "line3", "line4", "line5", "old_ending"];
        let new: Vec<&str> = vec!["line1", "line2", "line3", "line4", "line5", "new_ending"];
        let diff = compute_diff(&old, &new);
        assert_eq!(
            diff,
            vec![
                DiffLine::Keep("line1"),
                DiffLine::Keep("line2"),
                DiffLine::Keep("line3"),
                DiffLine::Keep("line4"),
                DiffLine::Keep("line5"),
                DiffLine::Remove("old_ending"),
                DiffLine::Add("new_ending"),
            ]
        );
    }
}
