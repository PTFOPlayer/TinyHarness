//! Shared visual primitives for the *tool frame* family.
//!
//! Every piece of tool I/O — the execution card, the diff previews and the
//! confirmation prompt — is drawn from these parts so all three read as one
//! language:
//!
//! ```text
//!   ● run · cargo test --all · 1.2s        ← filled title band (`write_band`)
//!     │ running 199 tests                  ← outlined gutter (`gutter_line`)
//!     ╰ +40 more lines                     ← closing elbow marks the last row
//! ```
//!
//! Rules the family follows:
//! - **One filled band per block, and only for the title.** Whole-line
//!   background fills on body rows are loud and make output hard to scan;
//!   the band anchors the eye, the gutter carries the structure.
//! - **The gutter always closes with `╰`.** The elbow tells you where a block
//!   ends without needing a blank line or a rule.
//! - **Status lives in the glyph, not the body.** `●`/`✕`/`⊘` in
//!   [`FG_OK`]/[`FG_ERR`]/[`FG_WARN`] on the title band; body rows stay muted
//!   so the exception (an error line) is the only thing that can be loud.

use std::io::{self, Write};

use crate::style::*;

/// Left margin of every tool frame line. Two columns, matching the assistant
/// prose indent, so tool output nests visually under the message that asked
/// for it.
pub const INDENT: &str = "  ";
/// Gutter glyph for body rows.
pub const GUTTER: &str = "│";
/// Gutter glyph marking a gap between two hunks of the same block (skipped
/// context lines in a diff, hidden middle rows).
pub const GAP: &str = "┆";
/// Gutter glyph closing the last row of a block.
pub const FOOT: &str = "╰";

/// Columns consumed by `INDENT + GUTTER + " "` — subtract from the line budget
/// to get the space available for body text.
pub const GUTTER_WIDTH: usize = 4;

/// Truncate `s` to at most `max` *characters*, appending `…` when truncation
/// occurred.
///
/// Character-based (not byte-based) so a multibyte string is cut to the number
/// of columns it will actually occupy.
pub fn truncate_ellipsis(s: &str, max: usize) -> String {
    if s.char_indices().nth(max).is_none() {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    kept + "…"
}

/// Write one filled title band from pre-composed styled content, on a
/// specific background.
///
/// `styled` may contain its own escape sequences; any embedded [`RESET`]
/// clears the background mid-line, so the background is re-opened before the
/// fill to guarantee the band always reaches the margin.
fn band_line_on<W: Write>(w: &mut W, bg: &str, styled: &str) -> io::Result<()> {
    writeln!(w, "{bg}{INDENT}{styled}{bg}{FILL_EOL}{RESET}")
}

/// Write a filled title band from `(text, style)` segments, on a specific
/// background.
///
/// The background is restored between segments so the band is always
/// continuous — an embedded [`RESET`] clears the background as well as the
/// color, which is the easy thing to get wrong when hand-building bands.
pub fn write_band_on<W: Write>(w: &mut W, bg: &str, segments: &[(&str, &str)]) -> io::Result<()> {
    let mut styled = String::new();
    for (text, style) in segments {
        styled.push_str(&format!("{style}{text}{RESET}{bg}"));
    }
    band_line_on(w, bg, &styled)
}

/// Write a filled title band on the standard card background.
pub fn write_band<W: Write>(w: &mut W, segments: &[(&str, &str)]) -> io::Result<()> {
    write_band_on(w, BG_BAND, segments)
}

/// Write one gutter row of a tool frame.
///
/// Pass [`GUTTER`] for every row except the last, and [`FOOT`] for the last,
/// so the block reads as a closed shape. `styled` is the row content, already
/// carrying whatever inline styling it needs.
pub fn gutter_line<W: Write>(w: &mut W, glyph: &str, styled: &str) -> io::Result<()> {
    writeln!(w, "{INDENT}{FG_FAINT}{glyph}{RESET} {styled}{RESET}")
}

/// Write a single status line (no card body), for signal tools and other
/// one-line outcomes that share the card glyph vocabulary.
pub fn status_line<W: Write>(w: &mut W, glyph: &str, color: &str, text: &str) -> io::Result<()> {
    writeln!(w, "{INDENT}{color}{glyph}{RESET} {FG_MUTED}{text}{RESET}")
}

/// `deploy_to_staging` → `Deploy to staging`.
///
/// Human-facing labels (the confirmation prompt's description of an unknown
/// tool) should read like a sentence, not like a wire-format identifier:
/// words separated, only the first letter capitalized.
pub fn title_case(name: &str) -> String {
    let sentence = name
        .split(['_', '-'])
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = sentence.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(s: &str) -> String {
        let re = regex::Regex::new(r"\x1b\[[0-9;]*[mK]").unwrap();
        re.replace_all(s, "").to_string()
    }

    fn render_gutter(glyph: &str, text: &str) -> String {
        let mut buf = Vec::new();
        gutter_line(&mut buf, glyph, &format!("{FG_FAINT}{text}{RESET}")).unwrap();
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn gutter_row_is_indented_and_reset() {
        let out = render_gutter(GUTTER, "hello");
        assert_eq!(strip_ansi(&out), "  │ hello\n");
        assert!(out.ends_with(&format!("{RESET}\n")));
    }

    #[test]
    fn gutter_width_matches_the_rendered_prefix() {
        let out = render_gutter(GUTTER, "");
        let prefix = strip_ansi(&out);
        assert_eq!(prefix.chars().count(), GUTTER_WIDTH + 1);
    }

    #[test]
    fn band_fills_to_end_of_line() {
        let mut buf = Vec::new();
        write_band(&mut buf, &[("title", FG_BAND)]).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(strip_ansi(&out), "  title\n");
        assert!(out.contains(BG_BAND));
        assert!(out.contains(FILL_EOL));
    }

    #[test]
    fn segmented_band_keeps_the_background_continuous() {
        let mut buf = Vec::new();
        write_band(&mut buf, &[("●", FG_OK), (" run", FG_MUTED)]).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(strip_ansi(&out), "  ● run\n");
        // Every RESET that ends a segment must be followed by the background
        // again (or the line-ending fill), or the band would show holes. The
        // final RESET legitimately ends the line, so it is not inspected.
        let chunks: Vec<&str> = out.split("\x1b[0m").collect();
        for chunk in &chunks[1..chunks.len() - 1] {
            assert!(
                chunk.starts_with(BG_BAND) || chunk.starts_with(FILL_EOL),
                "background must reopen after each segment, got {chunk:?}"
            );
        }
    }

    #[test]
    fn title_case_humanizes_identifiers() {
        assert_eq!(title_case("deploy_to_staging"), "Deploy to staging");
        assert_eq!(title_case("run"), "Run");
        assert_eq!(title_case("my-tool"), "My tool");
        assert_eq!(title_case("__"), "");
    }

    #[test]
    fn truncate_ellipsis_is_char_boundary_safe() {
        let s = "żółw".repeat(50);
        let t = truncate_ellipsis(&s, 10);
        assert!(t.ends_with('…'));
        assert_eq!(t.chars().count(), 10);
    }

    #[test]
    fn truncate_ellipsis_leaves_short_text_alone() {
        assert_eq!(truncate_ellipsis("short", 10), "short");
        assert_eq!(truncate_ellipsis("exact", 5), "exact");
    }
}
