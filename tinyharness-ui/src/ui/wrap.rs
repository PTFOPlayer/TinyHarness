use std::{error::Error, io::Write};

use crate::style::FILL_EOL;

/// Maximum line width for word-wrapped output.
pub const MAX_LINE_WIDTH: usize = 120;

/// Break a single logical `line` into display rows that fit the available
/// width, splitting at word boundaries where possible and hard-breaking only
/// when a single word exceeds the budget.
///
/// `first_avail` / `rest_avail` are the columns left for text on the first row
/// and on continuation rows (i.e. `max_width - indent.len()`), which lets
/// hanging-indent layouts wrap without the continuation rows overflowing.
///
/// Always returns at least one row for a non-empty line, and exactly one empty
/// row for an empty line, so callers can round-trip blank lines.
pub fn wrap_line(line: &str, first_avail: usize, rest_avail: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    if line.chars().count() <= first_avail {
        return vec![line.to_string()];
    }

    let mut rows = Vec::new();
    let mut remaining = line;
    let mut avail = first_avail;
    loop {
        if remaining.chars().count() <= avail {
            rows.push(remaining.to_string());
            break;
        }
        let chunk_end = remaining.floor_char_boundary(avail);
        let chunk = &remaining[..chunk_end];
        match chunk.rfind(' ') {
            Some(pos) if pos > 0 => {
                rows.push(chunk[..pos].to_string());
                remaining = remaining[pos..].trim_start();
            }
            // No break point inside the budget — hard-break at the limit.
            _ => {
                rows.push(chunk.to_string());
                remaining = remaining[chunk_end..].trim_start();
            }
        }
        avail = rest_avail;
    }
    rows
}

/// Write lines to stdout with word-wrapping at `max_width` characters.
/// Lines shorter than `max_width - indent.len()` are written as-is.
/// Empty lines produce just a newline.
/// When `fill_bg` is true, each line is suffixed with `FILL_EOL` to fill
/// the background color to the end of the terminal line.
pub fn write_wrapped_lines<W: Write>(
    stdout: &mut W,
    content: &str,
    base_indent: &str,
    cont_indent: &str,
    max_width: usize,
    fill_bg: bool,
) -> Result<(), Box<dyn Error>> {
    let suffix = if fill_bg { FILL_EOL } else { "" };
    let first_avail = max_width.saturating_sub(base_indent.chars().count());
    let rest_avail = max_width.saturating_sub(cont_indent.chars().count());
    for line in content.lines() {
        for (i, row) in wrap_line(line, first_avail, rest_avail)
            .into_iter()
            .enumerate()
        {
            let indent = if i == 0 { base_indent } else { cont_indent };
            writeln!(stdout, "{indent}{row}{suffix}")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(line: &str, avail: usize) -> Vec<String> {
        wrap_line(line, avail, avail)
    }

    #[test]
    fn short_line_is_one_row() {
        assert_eq!(rows("hello", 10), vec!["hello"]);
    }

    #[test]
    fn empty_line_yields_one_empty_row() {
        assert_eq!(rows("", 10), vec![String::new()]);
    }

    #[test]
    fn wraps_at_word_boundaries() {
        assert_eq!(rows("alpha beta gamma", 12), vec!["alpha beta", "gamma"]);
    }

    #[test]
    fn hard_breaks_when_a_word_exceeds_the_budget() {
        let out = rows(&"x".repeat(25), 10);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], "x".repeat(10));
        assert_eq!(out.concat(), "x".repeat(25));
    }

    #[test]
    fn continuation_budget_is_honoured() {
        // First row is narrow, continuations wide: nothing may exceed its own
        // budget, and no text may be lost.
        let line = "aaa bbb ccc ddd eee";
        let out = wrap_line(line, 7, 12);
        assert!(out[0].len() <= 7);
        for rest in &out[1..] {
            assert!(rest.len() <= 12);
        }
        assert_eq!(out.join(" ").replace(' ', ""), line.replace(' ', ""));
    }

    #[test]
    fn multibyte_input_never_panics_and_loses_nothing() {
        let line = "🎉 ".repeat(10);
        let out = wrap_line(&line, 6, 6);
        assert_eq!(out.join(" ").replace(' ', ""), line.replace(' ', ""));
    }

    #[test]
    fn writer_pads_every_row_when_requested() {
        let mut buf = Vec::new();
        write_wrapped_lines(&mut buf, "one two three", "  ", "  ", 9, true).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.matches(FILL_EOL).count(), out.lines().count());
        assert!(out.lines().all(|l| l.len() <= 12), "{out:?}");
    }

    #[test]
    fn writer_keeps_blank_lines_blank() {
        let mut buf = Vec::new();
        write_wrapped_lines(&mut buf, "a\n\nb", "", "", 80, false).unwrap();
        assert_eq!(String::from_utf8(buf).unwrap(), "a\n\nb\n");
    }
}
