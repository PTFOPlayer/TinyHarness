//! Compact, styled rendering of tool execution cards.
//!
//! Shared by the live agent loop (after a tool finishes executing) and the
//! session-history replay (when a saved session is printed), so both paths
//! render identically.
//!
//! A card is one filled title band naming the call — status glyph, tool,
//! subject, duration — followed by a gutter-outlined body:
//!
//! ```text
//!   ● run · cargo test --all · 1.2s
//!     │ running 199 tests
//!     ╰ +40 more lines
//! ```
//!
//! Tool results are *plain text*, not markdown — they are raw command output,
//! file listings and API responses emitted verbatim by the model. Interpreting
//! them as markdown would corrupt `**` in test output, `-` in diffs and `#` in
//! shell comments, so this module uses a small structural renderer instead,
//! following the same design rules as [`super::markdown`] and the layout
//! primitives in [`super::frame`]:
//!
//! - **Never lose the summary.** Long output is capped, but the footer states
//!   exactly how many lines were hidden (`╰ +N more lines`).
//! - **The title band always says what was acted on**, so a reader scrolling
//!   back learns which file or command a card refers to without opening the
//!   body.
//! - **Errors are always visible in full prominence**, on a single red line.
//! - **Only the status glyph carries strong color** in the header; body rows
//!   stay muted, so an error line is the one thing allowed to be loud.

use std::io::{self, Write};

use super::frame::{FOOT, GUTTER, GUTTER_WIDTH, gutter_line, truncate_ellipsis, write_band};
use super::wrap::{MAX_LINE_WIDTH, wrap_line};
use crate::style::*;

/// Maximum body lines shown for free-form output (`run`, tool dumps).
const MAX_OUTPUT_LINES: usize = 10;
/// Maximum content lines shown after a `read` meta line.
const MAX_READ_LINES: usize = 3;
/// Maximum entries shown for parsed `web_search` output.
const MAX_SEARCH_RESULTS: usize = 5;
/// Columns left for body text inside a gutter row.
const BODY_WIDTH: usize = MAX_LINE_WIDTH - GUTTER_WIDTH;
/// Maximum characters for the one-line subject on the title band. The band
/// fills the terminal, so it is not wrapped and only needs a sanity cap.
const MAX_SUBJECT_CHARS: usize = 120;

/// Signal tools produce conversational meta-results ("User answered …"),
/// not raw output — always render as a single dim line.
const SIGNAL_TOOLS: [&str; 4] = ["question", "switch_mode", "invoke_skill", "auto_compact"];

/// Prefix of the denial message the agent loop records for a refused call
/// (see `src/agent/tools.rs`). Its content is fully implied by the card's
/// title band, so it is not repeated in the body.
const DENIED_BOILERPLATE: &str = "[Tool denied] The user denied the";

/// Where a tool call is in its lifecycle, and how it ended.
///
/// The status decides the title band's glyph and color — the only strongly
/// colored element on the line — so a user scanning a scrolled-back transcript
/// can tell success from failure from refusal at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    /// Completed without error.
    Ok,
    /// Returned an error.
    Error,
    /// The user refused it — nothing ran.
    Denied,
}

impl ToolStatus {
    /// Status glyph drawn on the title band.
    ///
    /// The braille spinner frames are deliberately not reused here: the live
    /// spinner is drawn on this same line while the call runs and would fight
    /// a static glyph for the same cell.
    fn glyph(self) -> &'static str {
        match self {
            ToolStatus::Ok => "●",
            ToolStatus::Error => "✕",
            ToolStatus::Denied => "⊘",
        }
    }

    fn color(self) -> &'static str {
        match self {
            ToolStatus::Ok => FG_OK,
            ToolStatus::Error => FG_ERR,
            ToolStatus::Denied => FG_WARN,
        }
    }

    /// Trailing label for terminal states that have no duration to show.
    fn label(self) -> &'static str {
        match self {
            ToolStatus::Error => "failed",
            ToolStatus::Denied => "denied",
            _ => "",
        }
    }
}

/// Format a duration in milliseconds as a human-readable string.
///
/// Under 1 second: `42ms` — 1–59 seconds: `1.2s` — 60+: `1m 23s`.
pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{mins}m {secs}s")
    }
}

/// Render one tool card: a status title band plus a capped, tool-specific
/// gutter body.
///
/// - `tool`: tool name (e.g. `read`, `run`, `web_search`).
/// - `subject`: what the call acted on — the command, the path, the query.
///   Pass `None` when it can't be determined (history replay, argument-less
///   tools) and the band degrades to just the tool name.
/// - `status`: see [`ToolStatus`].
/// - `duration_ms`: execution duration, when known (`None` for history
///   replay, where durations are not persisted).
/// - `body`: the raw tool result, without any `### {tool}` prefix.
pub fn write_tool_card<W: Write>(
    w: &mut W,
    tool: &str,
    subject: Option<&str>,
    status: ToolStatus,
    duration_ms: Option<u64>,
    body: &str,
) -> io::Result<()> {
    write_card_header(w, tool, subject, status, duration_ms)?;
    if body.trim().is_empty() {
        return Ok(());
    }
    write_body_lines(w, &card_lines(tool, body, status))
}

/// The call's primary target — command, path or query — for a card's title
/// band.
///
/// Recognized argument names are checked in order of specificity, then any
/// first string value, so custom tools still say what they were pointed at.
/// Returns `None` when the call has no string arguments.
pub fn tool_subject(arguments: &serde_json::Value) -> Option<String> {
    let args = arguments.as_object()?;
    for key in ["command", "path", "file_path", "query", "url", "pattern"] {
        if let Some(value) = args.get(key).and_then(|v| v.as_str())
            && !value.trim().is_empty()
        {
            return Some(value.lines().next().unwrap_or_default().to_string());
        }
    }
    args.values()
        .find_map(|v| v.as_str())
        .map(|v| v.lines().next().unwrap_or_default().to_string())
        .filter(|v| !v.trim().is_empty())
}

/// Write a card's title band: status glyph, tool name, subject, duration.
fn write_card_header<W: Write>(
    w: &mut W,
    tool: &str,
    subject: Option<&str>,
    status: ToolStatus,
    duration_ms: Option<u64>,
) -> io::Result<()> {
    // Compose the optional trailing pieces before assembling the band, so the
    // segments can all borrow from values that outlive the call.
    let subject_text = subject
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| truncate_ellipsis(s.lines().next().unwrap_or_default(), MAX_SUBJECT_CHARS));

    let tail = match (status, duration_ms) {
        (_, Some(ms)) => Some((format!(" · {}", format_duration(ms)), FG_FAINT)),
        // Without a duration (history replay) the terminal states still need
        // to say how they ended.
        (ToolStatus::Error | ToolStatus::Denied, None) => {
            Some((format!(" · {}", status.label()), status.color()))
        }
        (ToolStatus::Ok, None) => None,
    };

    let mut segments: Vec<(&str, &str)> = vec![(status.glyph(), status.color()), (" ", FG_FAINT)];
    segments.push((tool, FG_BAND));
    if let Some(subject) = &subject_text {
        segments.push((" · ", FG_FAINT));
        segments.push((subject.as_str(), FG_ACCENT));
    }
    if let Some((text, color)) = &tail {
        segments.push((text.as_str(), *color));
    }
    write_band(w, &segments)
}

/// One physical line of a card body, before framing.
enum BodyLine {
    /// Plain text, painted in a single color.
    Text(String, &'static str),
    /// Pre-styled content with its own inline colors (search entries).
    Rich(String),
}

/// Frame and write body lines: every line but the last uses the [`GUTTER`]
/// bar, the last closes the card with the [`FOOT`] elbow.
fn write_body_lines<W: Write>(w: &mut W, lines: &[BodyLine]) -> io::Result<()> {
    let last = lines.len().saturating_sub(1);
    for (i, line) in lines.iter().enumerate() {
        let glyph = if i == last { FOOT } else { GUTTER };
        match line {
            BodyLine::Text(text, color) => gutter_line(w, glyph, &format!("{color}{text}{RESET}"))?,
            BodyLine::Rich(styled) => gutter_line(w, glyph, styled)?,
        }
    }
    Ok(())
}

/// Append `text` to `lines` as however many physical rows it needs at the
/// body width.
///
/// Embedded newlines (a summary joining several entries, say) become separate
/// rows rather than one row that would silently break the gutter framing.
fn push_wrapped(lines: &mut Vec<BodyLine>, text: &str, color: &'static str) {
    for segment in text.split('\n') {
        for row in wrap_line(segment, BODY_WIDTH, BODY_WIDTH) {
            lines.push(BodyLine::Text(row, color));
        }
    }
}

/// Append the hidden-rows footer.
fn push_more(lines: &mut Vec<BodyLine>, hidden: usize, noun: &'static str) {
    if hidden > 0 {
        let noun = if hidden == 1 { singular(noun) } else { noun };
        lines.push(BodyLine::Text(format!("+{hidden} more {noun}"), FG_FAINT));
    }
}

/// `"lines"` → `"line"`, `"results"` → `"result"`.
fn singular(noun: &str) -> &'static str {
    match noun {
        "lines" => "line",
        "results" => "result",
        _ => "line",
    }
}

/// Build the body lines of a card for `tool`/`body`.
fn card_lines(tool: &str, body: &str, status: ToolStatus) -> Vec<BodyLine> {
    let mut lines: Vec<BodyLine> = Vec::new();

    if status == ToolStatus::Error {
        // The header already says `failed`; the body carries the message.
        push_wrapped(
            &mut lines,
            &truncate_ellipsis(body.lines().next().unwrap_or("Error"), BODY_WIDTH),
            FG_ERR,
        );
        return lines;
    }
    if body.starts_with("[Tool denied]") {
        // The canonical denial template only restates what the band already
        // says (tool, subject, `denied`), so it renders as a band alone. Any
        // other message is shown with the marker stripped.
        if body.starts_with(DENIED_BOILERPLATE) {
            return lines;
        }
        let msg = body.trim_start_matches("[Tool denied]").trim();
        push_wrapped(&mut lines, &truncate_ellipsis(msg, BODY_WIDTH), FG_WARN);
        return lines;
    }

    match tool {
        // Listings collapse to a single summary line.
        "ls" | "grep" | "glob" => push_wrapped(
            &mut lines,
            &truncate_ellipsis(&summarize_listing_result(body, tool), BODY_WIDTH),
            FG_MUTED,
        ),
        // Structured search results render as a link-styled list.
        "web_search" => match parse_web_search(body) {
            Some(entries) => search_lines(&mut lines, &entries),
            None => capped_lines(&mut lines, body.lines(), MAX_OUTPUT_LINES),
        },
        // File reads show the line-count meta line, then a small preview.
        "read" => read_lines(&mut lines, body),
        // Conversational meta-results collapse to one dim line.
        t if SIGNAL_TOOLS.contains(&t) => push_wrapped(
            &mut lines,
            &truncate_ellipsis(body.lines().next().unwrap_or(body), BODY_WIDTH),
            FG_MUTED,
        ),
        // Everything else: capped raw output.
        _ => capped_lines(&mut lines, body.lines(), MAX_OUTPUT_LINES),
    }
    lines
}

/// Show up to `max` of `body_lines`, noting how many were hidden.
fn capped_lines<'a>(
    out: &mut Vec<BodyLine>,
    body_lines: impl Iterator<Item = &'a str>,
    max: usize,
) {
    let all: Vec<&str> = body_lines.collect();
    for line in all.iter().take(max) {
        push_wrapped(out, line, FG_MUTED);
    }
    push_more(out, all.len().saturating_sub(max), "lines");
}

/// Meta line, then up to [`MAX_READ_LINES`] content lines and a footer.
fn read_lines(out: &mut Vec<BodyLine>, body: &str) {
    let mut lines = body.lines();
    let first = lines.next().unwrap_or("");
    let is_meta = first.starts_with('(') && first.contains(" lines") && first.ends_with(')');

    let (meta, content): (Option<&str>, Vec<&str>) = if is_meta {
        (Some(first), lines.collect())
    } else {
        (None, std::iter::once(first).chain(lines).collect())
    };

    if let Some(meta) = meta {
        out.push(BodyLine::Text(meta.to_string(), FG_FAINT));
    }
    for line in content.iter().take(MAX_READ_LINES) {
        push_wrapped(out, &truncate_ellipsis(line, BODY_WIDTH), FG_MUTED);
    }
    push_more(out, content.len().saturating_sub(MAX_READ_LINES), "lines");
}

/// Render parsed search entries as a link-styled list (matching the markdown
/// renderer's link treatment: accent title, faint URL).
fn search_lines(out: &mut Vec<BodyLine>, entries: &[SearchEntry]) {
    for entry in entries.iter().take(MAX_SEARCH_RESULTS) {
        out.push(BodyLine::Rich(format!(
            "{FG_FAINT}•{RESET} {FG_ACCENT}{}{RESET} {FG_FAINT}{}{RESET}",
            truncate_ellipsis(&entry.title, 60),
            truncate_ellipsis(&format!("({})", entry.url), 60),
        )));
        let snippet = entry.content.lines().next().unwrap_or("");
        if !snippet.is_empty() {
            push_wrapped(
                out,
                &format!("  {}", truncate_ellipsis(snippet, 100)),
                FG_FAINT,
            );
        }
    }
    push_more(
        out,
        entries.len().saturating_sub(MAX_SEARCH_RESULTS),
        "results",
    );
}

/// One parsed `web_search` entry.
struct SearchEntry {
    title: String,
    url: String,
    content: String,
}

/// Parse the `web_search` tool's textual output format:
///
/// ```text
/// [1] Title
///     URL: https://…
///     content…
/// ```
///
/// Returns `None` when the body doesn't match the format at all, so callers
/// can fall back to raw capped output.
fn parse_web_search(body: &str) -> Option<Vec<SearchEntry>> {
    let mut entries: Vec<SearchEntry> = Vec::new();

    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("    URL: ") {
            // URL line completes the most recently started entry.
            let last = entries.last_mut()?;
            last.url = rest.trim().to_string();
        } else if line.starts_with('[')
            && let Some(close) = line.find("] ")
            && close > 1
            && line[1..close].chars().all(|c| c.is_ascii_digit())
        {
            entries.push(SearchEntry {
                title: line[close + 2..].trim().to_string(),
                url: String::new(),
                content: String::new(),
            });
        } else if let Some(last) = entries.last_mut() {
            // Continuation (snippet) lines accumulate into the open entry.
            // Blank separator lines between entries are skipped — otherwise
            // they leave a trailing space on the previous snippet.
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !last.content.is_empty() {
                last.content.push(' ');
            }
            last.content.push_str(trimmed);
        }
    }

    if entries.is_empty() || entries.iter().any(|e| e.url.is_empty()) {
        return None;
    }
    Some(entries)
}

/// Produce a one-line summary for listing tools (ls, grep, glob).
///
/// `5 entries — Cargo.toml, src, target ... (2 more)`. Always a single line:
/// the whole point is to collapse a listing that would otherwise bury the
/// cards around it.
pub fn summarize_listing_result(result: &str, tool_name: &str) -> String {
    if result.starts_with("Error:") || result.starts_with("No ") || result == "Directory is empty" {
        return result.to_string();
    }

    let lines: Vec<&str> = result.lines().collect();
    let count = lines.len();
    let label = match (tool_name, count == 1) {
        ("ls", true) => "entry",
        ("ls", false) => "entries",
        ("grep", true) => "match",
        ("grep", false) => "matches",
        ("glob", true) => "file",
        ("glob", false) => "files",
        (_, true) => "result",
        (_, false) => "results",
    };

    const PREVIEW: usize = 3;
    let preview = lines
        .iter()
        .take(PREVIEW)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    if count <= PREVIEW {
        format!("{count} {label} — {preview}")
    } else {
        format!("{count} {label} — {preview} ... ({} more)", count - PREVIEW)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn strip_ansi(s: &str) -> String {
        let re = regex::Regex::new(r"\x1b\[[0-9;]*[mK]").unwrap();
        re.replace_all(s, "").to_string()
    }

    /// Render a finished card (the common case in tests).
    fn render(tool: &str, body: &str, is_error: bool, ms: Option<u64>) -> String {
        let mut buf = Vec::new();
        write_tool_card(
            &mut buf,
            tool,
            None,
            if is_error {
                ToolStatus::Error
            } else {
                ToolStatus::Ok
            },
            ms,
            body,
        )
        .unwrap();
        String::from_utf8(buf).unwrap()
    }

    fn plain(s: &str) -> String {
        strip_ansi(s)
    }

    fn body_rows(out: &str) -> Vec<String> {
        plain(out).lines().skip(1).map(str::to_string).collect()
    }

    // ── Header ──

    #[test]
    fn success_header_shows_status_glyph_and_duration() {
        let out = render("read", "(1 lines)\nhi", false, Some(42));
        assert!(out.contains('●'));
        assert!(out.contains("read"));
        assert!(out.contains("42ms"));
        assert!(!out.contains('✕'));
        assert!(out.contains(FG_OK), "status color must be applied");
    }

    #[test]
    fn error_header_shows_cross_and_failure_label() {
        let mut buf = Vec::new();
        write_tool_card(
            &mut buf,
            "run",
            None,
            ToolStatus::Error,
            None,
            "Error: boom",
        )
        .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains('✕'));
        assert!(out.contains(FG_ERR));
        assert!(plain(&out).contains("failed"), "no duration ⇒ label");
    }

    #[test]
    fn error_with_duration_omits_the_label() {
        let out = render("run", "Error: boom", true, Some(1500));
        assert!(out.contains('✕'));
        assert!(out.contains("1.5s"));
        assert!(!plain(&out).contains("failed"));
    }

    #[test]
    fn denied_header_shows_slash_circle_and_denied_label() {
        let mut buf = Vec::new();
        write_tool_card(
            &mut buf,
            "run",
            Some("rm -rf /"),
            ToolStatus::Denied,
            None,
            "",
        )
        .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains('⊘'));
        assert!(plain(&out).contains("denied"));
        assert!(plain(&out).contains("rm -rf /"));
    }

    #[test]
    fn subject_appears_on_the_title_band() {
        let mut buf = Vec::new();
        write_tool_card(
            &mut buf,
            "read",
            Some("src/main.rs"),
            ToolStatus::Ok,
            None,
            "",
        )
        .unwrap();
        assert!(plain(&String::from_utf8(buf).unwrap()).contains("src/main.rs"));
    }

    #[test]
    fn multiline_subject_collapses_to_its_first_line() {
        let mut buf = Vec::new();
        write_tool_card(
            &mut buf,
            "run",
            Some("echo one\necho two"),
            ToolStatus::Ok,
            None,
            "",
        )
        .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.lines().count(), 1, "band must stay a single line");
        assert!(plain(&out).contains("echo one"));
        assert!(!plain(&out).contains("echo two"));
    }

    #[test]
    fn blank_subject_is_omitted() {
        let mut buf = Vec::new();
        write_tool_card(&mut buf, "ls", Some("   "), ToolStatus::Ok, None, "").unwrap();
        assert_eq!(plain(&String::from_utf8(buf).unwrap()).trim(), "● ls");
    }

    #[test]
    fn history_omits_duration() {
        let out = render("ls", "a\nb", false, None);
        assert!(!plain(&out).contains('·'), "no separator: {out:?}");
        assert!(out.contains("ls"));
    }

    #[test]
    fn empty_body_is_header_only() {
        let out = render("run", "", false, Some(5));
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn whitespace_only_body_is_header_only() {
        let out = render("run", "   \n  ", false, None);
        assert_eq!(out.lines().count(), 1);
    }

    // ── Body framing ──

    #[test]
    fn body_rows_are_gutter_outlined_and_close_with_an_elbow() {
        let out = render("run", "one\ntwo", false, None);
        let rows = body_rows(&out);
        assert_eq!(rows.len(), 2, "band + 2 body rows");
        assert!(rows[0].starts_with("  │ "), "body rows use the gutter");
        assert!(rows[1].starts_with("  ╰ "), "last row closes the card");
        assert!(rows[0].contains("one"));
        assert!(rows[1].contains("two"));
    }

    #[test]
    fn single_row_body_closes_with_the_elbow() {
        let out = render("ls", "a\nb\nc", false, None);
        let rows = body_rows(&out);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].starts_with("  ╰ "));
    }

    #[test]
    fn elbow_lands_on_the_final_physical_row_after_wrapping() {
        let line = "word ".repeat(80);
        let out = render("run", line.trim(), false, None);
        let rows = body_rows(&out);
        assert!(rows.len() > 2, "long line must wrap onto several rows");
        assert!(rows.last().unwrap().starts_with("  ╰ "));
        for row in &rows[..rows.len() - 1] {
            assert!(row.starts_with("  │ "), "no early elbow: {row:?}");
        }
    }

    #[test]
    fn no_body_row_exceeds_the_line_budget() {
        let out = render("run", &"word ".repeat(60), false, None);
        for row in body_rows(&out) {
            assert!(row.chars().count() <= MAX_LINE_WIDTH, "overflow: {row:?}");
        }
    }

    // ── Error / denied bodies ──

    #[test]
    fn error_body_is_a_single_red_line_truncated() {
        let long = format!("Error: {}", "x".repeat(300));
        let out = render("run", &long, true, None);
        let rows = body_rows(&out);
        assert_eq!(rows.len(), 1, "error collapses to one row: {rows:?}");
        assert!(out.contains(FG_ERR));
        assert!(plain(&rows[0]).ends_with('…'));
        assert!(!plain(&out).contains(&"x".repeat(200)));
    }

    #[test]
    fn error_body_shows_only_the_first_line() {
        let out = render("run", "Error: boom\nstack trace\nmore", true, None);
        let p = plain(&out);
        assert!(p.contains("Error: boom"));
        assert!(!p.contains("stack trace"), "trace stays hidden: {p:?}");
    }

    #[test]
    fn canonical_denial_renders_band_only() {
        // The boilerplate message only restates the band (tool, subject,
        // `denied`), so repeating it in the body would be pure noise.
        let mut buf = Vec::new();
        write_tool_card(
            &mut buf,
            "run",
            Some("rm -rf /"),
            ToolStatus::Denied,
            None,
            "[Tool denied] The user denied the 'run' tool call with arguments: command=\"rm -rf /\"",
        )
        .unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out.lines().count(), 1, "no body row: {out:?}");
        assert!(out.contains('⊘'));
        assert!(out.contains(FG_WARN));
    }

    #[test]
    fn non_boilerplate_denial_shows_the_message_without_the_marker() {
        let out = render(
            "run",
            "[Tool denied] sandbox policy refuses writes outside the workspace",
            false,
            None,
        );
        let p = plain(&out);
        assert!(out.contains(FG_WARN));
        assert!(p.contains("sandbox policy"), "{p:?}");
        assert!(!p.contains("[Tool denied]"), "marker is redundant: {p:?}");
    }

    // ── Listing tools ──

    #[test]
    fn listing_summary_line() {
        let out = render("ls", "Cargo.toml\nsrc\ntarget\nx\ny", false, None);
        let p = plain(&out);
        assert!(p.contains("5 entries — Cargo.toml, src, target ... (2 more)"));
        assert_eq!(body_rows(&out).len(), 1);
    }

    #[test]
    fn listing_error_passes_through() {
        let out = render("grep", "Error: bad pattern", true, None);
        assert!(plain(&out).contains("Error: bad pattern"));
    }

    // ── read ──

    #[test]
    fn read_shows_meta_then_capped_preview() {
        let content: Vec<String> = (1..=20).map(|i| format!("line{i}")).collect();
        let body = format!("(20 lines)\n{}", content.join("\n"));
        let out = render("read", &body, false, None);
        let p = plain(&out);
        assert!(p.contains("(20 lines)"));
        assert!(p.contains("line1"));
        assert!(p.contains("line3"));
        assert!(!p.contains("line4"));
        assert!(p.contains("╰ +17 more lines"));
    }

    #[test]
    fn read_without_meta_line_renders_all_as_content() {
        let out = render("read", "just one line", false, None);
        let p = plain(&out);
        assert!(p.contains("just one line"));
        assert!(!p.contains("more lines"));
        assert_eq!(body_rows(&out).len(), 1);
    }

    // ── run / default capping ──

    #[test]
    fn run_output_capped_with_footer() {
        let content: Vec<String> = (1..=50).map(|i| format!("out{i}")).collect();
        let out = render("run", &content.join("\n"), false, Some(2300));
        let p = plain(&out);
        assert!(p.contains("out1"));
        assert!(p.contains("out10"));
        assert!(!p.contains("out11"));
        assert!(p.contains("╰ +40 more lines"));
        assert!(out.contains("2.3s"));
    }

    #[test]
    fn short_output_not_capped() {
        let out = render("run", "one\ntwo", false, None);
        assert!(!plain(&out).contains("more lines"));
    }

    // ── web_search ──

    fn search_body(n: usize) -> String {
        (1..=n)
            .map(|i| format!("[{i}] Title {i}\n    URL: https://x.dev/{i}\n    Snippet {i} text\n"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn web_search_renders_link_styled_list() {
        let out = render("web_search", &search_body(3), false, None);
        let p = plain(&out);
        assert!(p.contains("• Title 1"));
        assert!(p.contains("(https://x.dev/1)"));
        assert!(p.contains("Snippet 1 text"));
        assert!(out.contains(FG_ACCENT), "titles should be link-colored");
        assert!(!p.contains("more results"), "3 entries fit under the cap");
    }

    #[test]
    fn web_search_caps_results_with_footer() {
        let out = render("web_search", &search_body(8), false, None);
        let p = plain(&out);
        assert!(p.contains("Title 1"));
        assert!(p.contains("Title 5"));
        assert!(!p.contains("Title 6"));
        assert!(p.contains("╰ +3 more results"));
    }

    #[test]
    fn web_search_unparseable_falls_back_to_raw() {
        let out = render("web_search", "No results found.", false, None);
        assert!(plain(&out).contains("No results found."));
    }

    #[test]
    fn web_search_prose_starting_with_bracket_falls_back() {
        // `[1] x` without a following URL line is prose, not a result list.
        let out = render(
            "web_search",
            "[1] see section one of the manual",
            false,
            None,
        );
        assert!(plain(&out).contains("see section one"));
    }

    // ── signal tools ──

    #[test]
    fn signal_tool_result_is_single_dim_line() {
        let out = render(
            "question",
            "User answered the question 'Deploy?' with: 'yes'.",
            false,
            None,
        );
        assert_eq!(body_rows(&out).len(), 1);
        assert!(plain(&out).contains("User answered the question"));
    }

    // ── tool_subject ──

    #[test]
    fn subject_prefers_the_command_over_other_arguments() {
        assert_eq!(
            tool_subject(&json!({"command": "cargo build", "timeout": 30})).as_deref(),
            Some("cargo build")
        );
    }

    #[test]
    fn subject_prefers_the_path_for_file_tools() {
        assert_eq!(
            tool_subject(&json!({"path": "src/main.rs", "old_str": "a"})).as_deref(),
            Some("src/main.rs")
        );
    }

    #[test]
    fn subject_falls_back_to_the_first_string_argument() {
        assert_eq!(
            tool_subject(&json!({"count": 3, "target": "prod"})).as_deref(),
            Some("prod")
        );
    }

    #[test]
    fn subject_collapses_multiline_arguments_to_one_line() {
        assert_eq!(
            tool_subject(&json!({"command": "echo one\necho two"})).as_deref(),
            Some("echo one")
        );
    }

    #[test]
    fn subject_is_none_without_string_arguments() {
        assert_eq!(tool_subject(&json!({})), None);
        assert_eq!(tool_subject(&json!({"n": 1})), None);
        assert_eq!(tool_subject(&json!([])), None);
    }

    // ── format_duration ──

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(42), "42ms");
        assert_eq!(format_duration(1200), "1.2s");
        assert_eq!(format_duration(83_000), "1m 23s");
    }

    // ── summarize_listing_result ──

    #[test]
    fn summarize_labels_per_tool() {
        assert_eq!(
            summarize_listing_result("a\nb\nc", "grep"),
            "3 matches — a, b, c"
        );
        assert_eq!(summarize_listing_result("a", "ls"), "1 entry — a");
        assert_eq!(summarize_listing_result("a\nb", "glob"), "2 files — a, b");
    }

    #[test]
    fn summarize_is_always_a_single_line() {
        // A multi-entry summary containing newlines would break the gutter
        // framing of the card body, so the join is always ", ".
        let out = summarize_listing_result("a\nb\nc\nd\ne", "ls");
        assert!(!out.contains('\n'), "{out:?}");
        assert_eq!(out, "5 entries — a, b, c ... (2 more)");
    }

    #[test]
    fn summarize_error_passthrough() {
        assert_eq!(summarize_listing_result("Error: nope", "ls"), "Error: nope");
    }
}
