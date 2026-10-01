use std::{
    error::Error,
    io::{self, Write},
};

use super::frame::{FOOT, GAP, GUTTER, GUTTER_WIDTH, gutter_line, truncate_ellipsis, write_band};
use super::wrap::MAX_LINE_WIDTH;
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

// ── Public API ─────────────────────────────────────────────────────────────

/// Diff-card glyph — "not equal", marking the block as a change preview
/// rather than a completed tool result.
const DIFF_GLYPH: &str = "≠";

/// Which side of the diff a rendered row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Keep,
    Remove,
    Add,
}

impl Kind {
    /// One-character change indicator, in the column before the line number.
    fn sign(self) -> char {
        match self {
            Kind::Keep => ' ',
            Kind::Remove => '-',
            Kind::Add => '+',
        }
    }

    fn color(self) -> &'static str {
        match self {
            Kind::Keep => FG_FAINT,
            Kind::Remove => FG_ERR,
            Kind::Add => FG_OK,
        }
    }
}

/// One row of a diff card.
enum DiffRow {
    /// A source line, with the number of the side it belongs to.
    Line {
        kind: Kind,
        num: usize,
        text: String,
    },
    /// Hunk boundary: how many unchanged rows were skipped.
    Gap(usize),
}

/// Number every row of a computed diff with the line number of its own side,
/// so a reader can jump straight to the location in their editor.
fn numbered(diff: &[DiffLine]) -> Vec<(Kind, usize, String)> {
    let (mut old, mut new) = (0usize, 0usize);
    diff.iter()
        .map(|item| match item {
            DiffLine::Keep(t) => {
                old += 1;
                new += 1;
                (Kind::Keep, old, (*t).to_string())
            }
            DiffLine::Remove(t) => {
                old += 1;
                (Kind::Remove, old, (*t).to_string())
            }
            DiffLine::Add(t) => {
                new += 1;
                (Kind::Add, new, (*t).to_string())
            }
        })
        .collect()
}

/// Collapse numbered rows into hunks around the changes, keeping `context`
/// unchanged lines on each side and replacing everything else with a
/// [`DiffRow::Gap`] that says how much was hidden.
///
/// Never hides a change — only context — so the preview always shows every
/// line the tool would touch.
fn hunked(rows: &[(Kind, usize, String)], context: usize) -> Vec<DiffRow> {
    let changed: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, (k, ..))| *k != Kind::Keep)
        .map(|(i, _)| i)
        .collect();
    if changed.is_empty() {
        return vec![];
    }

    // Merge overlapping windows into hunk ranges.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    let mut start = changed[0].saturating_sub(context);
    let mut end = (changed[0] + context + 1).min(rows.len());
    for &i in &changed[1..] {
        let lo = i.saturating_sub(context);
        let hi = (i + context + 1).min(rows.len());
        if lo <= end {
            end = end.max(hi);
        } else {
            ranges.push((start, end));
            start = lo;
            end = hi;
        }
    }
    ranges.push((start, end));

    let mut out: Vec<DiffRow> = Vec::new();
    let mut cursor = 0usize;
    for (lo, hi) in ranges {
        if lo > cursor {
            out.push(DiffRow::Gap(lo - cursor));
        }
        for (kind, num, text) in &rows[lo..hi] {
            out.push(DiffRow::Line {
                kind: *kind,
                num: *num,
                text: text.clone(),
            });
        }
        cursor = hi;
    }
    out
}

/// Draw a diff card: a `≠` title band naming the operation and target, then
/// gutter-outlined rows carrying line numbers and `-`/`+` signs.
///
/// `stat` is a pre-styled trailing segment for the band (change counts, or a
/// note like `new file`); pass an empty string to omit it. When `rows` is
/// empty the card is just the band — the "nothing to change" case.
fn write_diff_card<W: Write>(
    w: &mut W,
    op: &str,
    path: &str,
    stat: &str,
    rows: &[DiffRow],
) -> io::Result<()> {
    // `stat` may carry its own inline colors (e.g. `-3 +7`), so it is passed
    // through as an unstyled segment.
    let mut segments: Vec<(&str, &str)> = vec![
        (DIFF_GLYPH, FG_ACCENT),
        (" ", FG_FAINT),
        (op, FG_BAND),
        (" · ", FG_FAINT),
        (path, FG_ACCENT),
    ];
    if !stat.is_empty() {
        segments.push((" · ", FG_FAINT));
        segments.push((stat, ""));
    }
    write_band(w, &segments)?;
    if rows.is_empty() {
        return Ok(());
    }

    let num_width = rows
        .iter()
        .map(|r| match r {
            DiffRow::Line { num, .. } => *num,
            DiffRow::Gap(_) => 0,
        })
        .max()
        .unwrap_or(1)
        .to_string()
        .len()
        .max(2);
    // Columns left for source text after the gutter, sign, number and space.
    let text_width = MAX_LINE_WIDTH.saturating_sub(GUTTER_WIDTH + num_width + 3);
    let last = rows.len() - 1;

    for (i, row) in rows.iter().enumerate() {
        match row {
            DiffRow::Line { kind, num, text } => {
                let glyph = if i == last { FOOT } else { GUTTER };
                let color = kind.color();
                gutter_line(
                    w,
                    glyph,
                    &format!(
                        "{color}{sign}{RESET}{FG_FAINT}{num:>width$}{RESET} {color}{text}{RESET}",
                        sign = kind.sign(),
                        num = num,
                        width = num_width,
                        text = truncate_ellipsis(text, text_width)
                    ),
                )?;
            }
            // Gap rows keep the open gutter — the card isn't finished yet.
            DiffRow::Gap(skipped) => {
                gutter_line(
                    w,
                    GAP,
                    &format!(
                        "{FG_FAINT}{skipped} unchanged lines{RESET}",
                        skipped = skipped
                    ),
                )?;
            }
        }
    }
    Ok(())
}

/// Count removed/added rows and format them as a band stat (`-3 +7`).
fn stat_of(diff: &[DiffLine]) -> String {
    let removals = diff
        .iter()
        .filter(|l| matches!(l, DiffLine::Remove(_)))
        .count();
    let additions = diff
        .iter()
        .filter(|l| matches!(l, DiffLine::Add(_)))
        .count();
    format!("{FG_ERR}-{removals}{RESET} {FG_OK}+{additions}{RESET}")
}

/// Show a unified-diff-style view of a write operation (full file).
/// If the file already exists, reads it and shows a line-by-line diff
/// with removed lines in red and added lines in green.
/// If the file is new, shows a preview of the content that will be written.
pub fn show_write_preview<W: Write>(
    stdout: &mut W,
    path: &str,
    new_content: &str,
) -> Result<(), Box<dyn Error>> {
    let new_lines: Vec<&str> = new_content.lines().collect();

    let Ok(existing) = std::fs::read_to_string(path) else {
        // File doesn't exist — every line is an addition.
        let rows: Vec<DiffRow> = new_lines
            .iter()
            .enumerate()
            .map(|(i, t)| DiffRow::Line {
                kind: Kind::Add,
                num: i + 1,
                text: (*t).to_string(),
            })
            .collect();
        let stat = format!(
            "{FG_OK}new file{RESET} {FG_FAINT}({} lines)",
            new_lines.len()
        );
        write_diff_card(stdout, "write", path, &stat, &rows)?;
        return Ok(());
    };

    let old_lines: Vec<&str> = existing.lines().collect();
    let diff = compute_diff(&old_lines, &new_lines);
    if !diff.iter().any(|l| !matches!(l, DiffLine::Keep(_))) {
        write_diff_card(stdout, "write", path, "no changes", &[])?;
        return Ok(());
    }

    let rows = hunked(&numbered(&diff), DIFF_CONTEXT_LINES);
    let stat = stat_of(&diff);
    write_diff_card(stdout, "write", path, &stat, &rows)?;
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
    let content = std::fs::read_to_string(path)
        .map_err(|e| -> Box<dyn Error> { format!("Failed to read '{path}': {e}").into() })?;

    // Find the byte offset of old_str in the content
    let Some(offset) = content.find(old_str) else {
        let rows = vec![DiffRow::Line {
            kind: Kind::Remove,
            num: 0,
            text: "'old_str' not found in file — the edit would fail".to_string(),
        }];
        return write_diff_card(stdout, "edit", path, "match failed", &rows).map_err(Into::into);
    };

    // Count newlines before the match to get the line number (0-based)
    let line_number = content[..offset].matches('\n').count();
    let lines: Vec<&str> = content.lines().collect();
    let old_lines: Vec<&str> = old_str.lines().collect();
    let new_lines: Vec<&str> = new_str.lines().collect();

    // Show a couple of lines of context around the replacement so the change
    // has a location, not just a content.
    let before_ctx = 2usize;
    let after_ctx = 2usize;
    let start_line = line_number.saturating_sub(before_ctx);

    let mut rows: Vec<DiffRow> = Vec::new();
    if start_line > 0 {
        rows.push(DiffRow::Gap(start_line));
    }
    for (i, line) in lines
        .iter()
        .enumerate()
        .take(line_number)
        .skip(start_line)
        .map(|(i, l)| (i + 1, l))
    {
        rows.push(DiffRow::Line {
            kind: Kind::Keep,
            num: i,
            text: line.to_string(),
        });
    }
    // The window is anchored on the old file, so removals and additions share
    // one number column — the reader sees both sides of the swap in place.
    for (i, line) in old_lines.iter().enumerate() {
        rows.push(DiffRow::Line {
            kind: Kind::Remove,
            num: line_number + i + 1,
            text: line.to_string(),
        });
    }
    for (i, line) in new_lines.iter().enumerate() {
        rows.push(DiffRow::Line {
            kind: Kind::Add,
            num: line_number + i + 1,
            text: line.to_string(),
        });
    }
    let after_start = line_number + old_lines.len();
    let end_line = (after_start + after_ctx).min(lines.len());
    for (i, line) in lines.iter().enumerate().take(end_line).skip(after_start) {
        rows.push(DiffRow::Line {
            kind: Kind::Keep,
            num: i + 1,
            text: line.to_string(),
        });
    }
    let tail = lines.len().saturating_sub(end_line);
    if tail > 0 {
        rows.push(DiffRow::Gap(tail));
    }

    let stat = format!(
        "{FG_ERR}-{n}{RESET} {FG_OK}+{m}{RESET}",
        n = old_lines.len(),
        m = new_lines.len()
    );
    write_diff_card(stdout, "edit", path, &stat, &rows)?;
    Ok(())
}

/// Render a [`DiffLine`] sequence into a plain-text string (no ANSI codes).
///
/// Returns a string with lines prefixed by `  ` (keep), `- ` (remove), or `+ ` (add),
/// and line numbers if requested.
pub fn render_diff_plain(
    old_lines: &[&str],
    new_lines: &[&str],
    diff: &[DiffLine],
    show_line_numbers: bool,
) -> String {
    if diff.is_empty() {
        return String::new();
    }

    let max_line = old_lines.len().max(new_lines.len()).max(1);
    let num_width = if show_line_numbers {
        max_line.to_string().len().max(2)
    } else {
        0
    };

    let mut result = String::new();
    let mut old_num: usize = 0;
    let mut new_num: usize = 0;

    // Find change positions to determine hunks with context
    let change_indices: Vec<usize> = diff
        .iter()
        .enumerate()
        .filter_map(|(i, l)| matches!(l, DiffLine::Remove(_) | DiffLine::Add(_)).then_some(i))
        .collect();

    if change_indices.is_empty() {
        return result;
    }

    // Merge overlapping/adjacent hunks
    let mut hunk_ranges: Vec<(usize, usize)> = Vec::new();
    let mut hunk_start = change_indices[0].saturating_sub(DIFF_CONTEXT_LINES);
    let mut hunk_end = (change_indices[0] + DIFF_CONTEXT_LINES + 1).min(diff.len());

    for &idx in &change_indices[1..] {
        let ns = idx.saturating_sub(DIFF_CONTEXT_LINES);
        let ne = (idx + DIFF_CONTEXT_LINES + 1).min(diff.len());
        if ns <= hunk_end {
            hunk_end = hunk_end.max(ne);
        } else {
            hunk_ranges.push((hunk_start, hunk_end));
            hunk_start = ns;
            hunk_end = ne;
        }
    }
    hunk_ranges.push((hunk_start, hunk_end));

    for (hunk_idx, &(start, end)) in hunk_ranges.iter().enumerate() {
        // Separator between hunks
        if hunk_idx > 0 {
            result.push_str("  ┈\n");
        }

        // Count line numbers up to the start of this hunk
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
                        result.push_str(&format!(
                            "  {:>width$} │ {}\n",
                            old_num,
                            line,
                            width = num_width
                        ));
                    } else {
                        result.push_str(&format!("    {}\n", line));
                    }
                }
                DiffLine::Remove(line) => {
                    old_num += 1;
                    if show_line_numbers {
                        result.push_str(&format!(
                            "-  {:>width$} │ {}\n",
                            old_num,
                            line,
                            width = num_width
                        ));
                    } else {
                        result.push_str(&format!("-   {}\n", line));
                    }
                }
                DiffLine::Add(line) => {
                    new_num += 1;
                    if show_line_numbers {
                        result.push_str(&format!(
                            "+  {:>width$} │ {}\n",
                            new_num,
                            line,
                            width = num_width
                        ));
                    } else {
                        result.push_str(&format!("+   {}\n", line));
                    }
                }
            }
        }
    }

    result
}

/// Compute a unified diff between old and new content and return it as
/// a plain-text string (no ANSI codes). Returns an empty string if the
/// contents are identical.
pub fn compute_edit_diff_plain(old_content: &str, new_content: &str) -> String {
    let old_lines: Vec<&str> = old_content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();
    let diff = compute_diff(&old_lines, &new_lines);

    if diff.iter().all(|l| matches!(l, DiffLine::Keep(_))) {
        return String::new();
    }

    render_diff_plain(&old_lines, &new_lines, &diff, true)
}

/// Compute a diff for a write operation (file creation or modification) and
/// return it as a plain-text string (no ANSI codes).
pub fn compute_write_diff_plain(path: &str, new_content: &str) -> String {
    let existing = std::fs::read_to_string(path);

    match existing {
        Ok(old_content) => {
            let old_lines: Vec<&str> = old_content.lines().collect();
            let new_lines: Vec<&str> = new_content.lines().collect();
            let diff = compute_diff(&old_lines, &new_lines);

            if diff.iter().all(|l| matches!(l, DiffLine::Keep(_))) {
                return String::new();
            }

            render_diff_plain(&old_lines, &new_lines, &diff, true)
        }
        Err(_) => {
            // New file — show all lines as additions
            let new_lines: Vec<&str> = new_content.lines().collect();
            let num_width = new_lines.len().to_string().len().max(2);
            let mut result = String::new();
            for (i, line) in new_lines.iter().enumerate() {
                result.push_str(&format!(
                    "+  {:>width$} │ {}\n",
                    i + 1,
                    line,
                    width = num_width
                ));
            }
            result
        }
    }
}

/// Compute a diff for an edit operation and return it as a plain-text string
/// (no ANSI codes). Reads the current file content to locate the edit.
pub fn compute_edit_diff_from_path(path: &str, old_str: &str, new_str: &str) -> String {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return String::new(),
    };

    let lines: Vec<&str> = content.lines().collect();

    // Find the line number of old_str
    let offset = match content.find(old_str) {
        Some(o) => o,
        None => return String::new(),
    };
    let line_number = content[..offset].matches('\n').count();

    let old_lines: Vec<&str> = old_str.lines().collect();
    let new_lines: Vec<&str> = new_str.lines().collect();

    // Show context (2 lines before and after)
    let before_ctx = 2usize;
    let after_ctx = 2usize;
    let start_line = line_number.saturating_sub(before_ctx);
    let end_line = (line_number + old_lines.len() + after_ctx).min(lines.len());
    let num_width = end_line.to_string().len().max(2);

    let mut result = String::new();

    // Context lines before
    for (i, line) in lines.iter().enumerate().take(line_number).skip(start_line) {
        result.push_str(&format!(
            "  {:>width$} │ {}\n",
            i + 1,
            line,
            width = num_width
        ));
    }

    // Removed lines (old)
    for line in old_lines.iter() {
        result.push_str(&format!("-       │ {}\n", line));
    }

    // Added lines (new)
    for line in new_lines.iter() {
        result.push_str(&format!("+       │ {}\n", line));
    }

    // Context lines after
    let after_start = line_number + old_lines.len();
    for i in after_start..end_line {
        if i < lines.len() {
            result.push_str(&format!(
                "  {:>width$} │ {}\n",
                i + 1,
                lines[i],
                width = num_width
            ));
        }
    }

    result
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
