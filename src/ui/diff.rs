use std::{error::Error, io::Write};

use crate::style::*;

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
            writeln!(stdout, "\n{}  ── New file: {} ──{}", BOLD, path, RESET)?;
            writeln!(stdout, "{}  {} {} {}", DIM, "    ", "┄".repeat(60), RESET)?;
            for line in new_content.lines() {
                writeln!(stdout, "{}  {}{}{}", GREEN, "+ ", RESET, line)?;
            }
            writeln!(stdout, "{}  {} {} {}", DIM, "    ", "┄".repeat(60), RESET)?;
            return Ok(());
        }
    };

    let new_lines: Vec<&str> = new_content.lines().collect();

    writeln!(stdout, "\n{}  ── Diff for {} ──{}", BOLD, path, RESET)?;
    writeln!(stdout, "{}  {} {} {}", DIM, "    ", "┄".repeat(60), RESET)?;

    // Simple line-by-line diff using LCS-like approach
    let mut old_idx = 0;
    let mut new_idx = 0;

    while old_idx < old_lines.len() || new_idx < new_lines.len() {
        if old_idx < old_lines.len()
            && new_idx < new_lines.len()
            && old_lines[old_idx] == new_lines[new_idx]
        {
            // Unchanged line (dim)
            writeln!(stdout, "  {} {}", DIM, old_lines[old_idx])?;
            old_idx += 1;
            new_idx += 1;
        } else {
            // Try to find the next matching line to determine what was removed/added
            let mut found = false;
            // Look ahead in old (possible removals)
            for lookahead in 1..=3 {
                if old_idx + lookahead < old_lines.len()
                    && new_idx < new_lines.len()
                    && old_lines[old_idx + lookahead] == new_lines[new_idx]
                {
                    for removed in old_idx..old_idx + lookahead {
                        writeln!(stdout, "{}  {}{}", RED, "- ", removed)?;
                    }
                    old_idx += lookahead;
                    found = true;
                    break;
                }
            }
            if !found {
                // Look ahead in new (possible additions)
                for lookahead in 1..=3 {
                    if new_idx + lookahead < new_lines.len()
                        && old_idx < old_lines.len()
                        && new_lines[new_idx + lookahead] == old_lines[old_idx]
                    {
                        for added in new_idx..new_idx + lookahead {
                            writeln!(stdout, "{}  {}{}", GREEN, "+ ", new_lines[added])?;
                        }
                        new_idx += lookahead;
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                // Can't align — treat as replace
                if old_idx < old_lines.len() {
                    writeln!(stdout, "{}  {}{}", RED, "- ", old_lines[old_idx])?;
                    old_idx += 1;
                }
                if new_idx < new_lines.len() {
                    writeln!(stdout, "{}  {}{}", GREEN, "+ ", new_lines[new_idx])?;
                    new_idx += 1;
                }
            }
        }
    }

    writeln!(stdout, "{}  {} {} {}", DIM, "    ", "┄".repeat(60), RESET)?;

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

    writeln!(stdout, "\n{}  ── Diff for {} ──{}", BOLD, path, RESET)?;

    // Separator line
    writeln!(
        stdout,
        "{}  {} {} {}",
        DIM,
        " ".repeat(line_num_width),
        "┄".repeat(60),
        RESET
    )?;

    // Lines before the change
    for i in start_line..line_number {
        writeln!(
            stdout,
            "  {:>width$} {} {}",
            i + 1,
            DIM,
            lines[i],
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
        "{}  {} {} {}",
        DIM,
        " ".repeat(line_num_width),
        "┄".repeat(60),
        RESET
    )?;

    Ok(())
}
