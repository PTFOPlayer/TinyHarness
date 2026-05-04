use std::{error::Error, io::Write};

/// Maximum line width for word-wrapped output.
pub const MAX_LINE_WIDTH: usize = 120;

/// Write lines to stdout with word-wrapping at `max_width` characters.
/// Lines shorter than `max_width - indent.len()` are written as-is.
/// Empty lines produce just a newline.
pub fn write_wrapped_lines<W: Write>(
    stdout: &mut W,
    content: &str,
    base_indent: &str,
    cont_indent: &str,
    max_width: usize,
) -> Result<(), Box<dyn Error>> {
    for line in content.lines() {
        if line.is_empty() {
            writeln!(stdout)?;
            continue;
        }
        if line.len() <= max_width - base_indent.len() {
            writeln!(stdout, "{}{}", base_indent, line)?;
            continue;
        }
        // Word-wrap long lines
        let mut remaining = line;
        let mut first = true;
        while !remaining.is_empty() {
            let indent = if first { base_indent } else { cont_indent };
            let avail = max_width - indent.len();
            if remaining.len() <= avail {
                writeln!(stdout, "{}{}", indent, remaining)?;
                break;
            }
            let chunk_end = remaining.floor_char_boundary(avail);
            let chunk = &remaining[..chunk_end];
            let split_at = chunk.rfind(' ').unwrap_or(chunk_end);
            if split_at == 0 {
                // No space found at all — just break at the width limit
                writeln!(stdout, "{}{}", indent, &remaining[..chunk_end])?;
                remaining = remaining[chunk_end..].trim_start();
            } else {
                writeln!(stdout, "{}{}", indent, &chunk[..split_at])?;
                remaining = remaining[chunk[..split_at].len()..].trim_start();
            }
            first = false;
        }
    }
    Ok(())
}
