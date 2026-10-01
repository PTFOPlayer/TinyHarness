//! Compact, styled rendering of tool execution results.
//!
//! Shared by the live agent loop (after a tool finishes executing) and the
//! session-history replay (when a saved session is printed), so both paths
//! render identically.
//!
//! Tool results are *plain text*, not markdown — they are raw command output,
//! file listings and API responses emitted verbatim by the model. Interpreting
//! them as markdown would corrupt `**` in test output, `-` in diffs and `#` in
//! shell comments, so this module uses a small structural renderer instead,
//! following the same design rules as [`super::markdown`]:
//!
//! - **Never lose the summary.** Long output is capped, but the footer states
//!   exactly how many lines were hidden (`┊ +N more lines`).
//! - **Errors are always visible in full prominence**, on a single red line.
//! - Styling is band-based (`BG_DIM` + `FILL_EOL`) so blocks read as one unit.

use std::io::{self, Write};

use super::wrap::{MAX_LINE_WIDTH, write_wrapped_lines};
use crate::style::*;

/// Maximum body lines shown for free-form output (`run`, tool dumps).
const MAX_OUTPUT_LINES: usize = 10;
/// Maximum content lines shown after a `read` meta line.
const MAX_READ_LINES: usize = 3;
/// Maximum entries shown for parsed `web_search` output.
const MAX_SEARCH_RESULTS: usize = 5;
/// Maximum characters for single-line summaries before truncation.
const MAX_SUMMARY_CHARS: usize = 120;

/// Signal tools produce conversational meta-results ("User answered …"),
/// not raw output — always render as a single dim line.
const SIGNAL_TOOLS: [&str; 4] = ["question", "switch_mode", "invoke_skill", "auto_compact"];

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

/// Truncate `s` to at most `max` characters (char-boundary safe), appending
/// `…` when truncation occurred.
fn truncate_ellipsis(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut = s.floor_char_boundary(max.saturating_sub(1));
    format!("{}…", &s[..cut])
}

/// Write one body line inside the `BG_DIM` band with a 6-space indent.
fn band_line<W: Write>(w: &mut W, text: &str, color: &str) -> io::Result<()> {
    writeln!(w, "{BG_DIM}      {color}{text}{FILL_EOL}{RESET}")
}

/// Render one tool result block: a status header line plus a capped,
/// tool-specific body.
///
/// - `tool`: tool name (e.g. `read`, `run`, `web_search`).
/// - `body`: the raw tool result, without any `### {tool}` prefix.
/// - `is_error`: whether the tool failed (colors the header `✗` red and
///   collapses the body to a single red line).
/// - `duration_ms`: execution duration, when known (`None` for history
///   replay, where durations are not persisted).
pub fn write_tool_result<W: Write>(
    w: &mut W,
    tool: &str,
    body: &str,
    is_error: bool,
    duration_ms: Option<u64>,
) -> io::Result<()> {
    let (icon, icon_color) = if is_error {
        ("✗", RED)
    } else {
        ("✓", GREEN)
    };
    let duration = duration_ms
        .map(format_duration)
        .map(|d| format!(" {DIM}· {d}{RESET}{BG_DIM}"))
        .unwrap_or_default();
    writeln!(
        w,
        "{BG_DIM}  {icon_color}{icon}{RESET}{BG_DIM} {DIM}{tool}{RESET}{BG_DIM}{duration}{FILL_EOL}{RESET}"
    )?;

    if body.trim().is_empty() {
        return Ok(());
    }

    if is_error {
        let msg = body.lines().next().unwrap_or("Error");
        return band_line(w, &truncate_ellipsis(msg, MAX_SUMMARY_CHARS), RED);
    }

    if body.starts_with("[Tool denied]") {
        let msg = body.lines().next().unwrap_or("");
        return band_line(w, &truncate_ellipsis(msg, MAX_SUMMARY_CHARS), ORANGE);
    }

    match tool {
        // Listings render as a single summary line ("3 entries — a, b, c").
        "ls" | "grep" | "glob" => band_line(
            w,
            &truncate_ellipsis(&summarize_listing_result(body, tool), MAX_SUMMARY_CHARS),
            DIM,
        ),
        // Structured search results render as a link-styled list.
        "web_search" => match parse_web_search(body) {
            Some(entries) => write_search_entries(w, &entries),
            None => write_capped_lines(w, body, MAX_OUTPUT_LINES),
        },
        // File reads show the line-count meta line, then a small preview.
        "read" => write_read_body(w, body),
        // Conversational meta-results collapse to one dim line.
        t if SIGNAL_TOOLS.contains(&t) => {
            let first = body.lines().next().unwrap_or(body);
            band_line(w, &truncate_ellipsis(first, MAX_SUMMARY_CHARS), DIM)
        }
        // Everything else: capped raw output.
        _ => write_capped_lines(w, body, MAX_OUTPUT_LINES),
    }
}

/// Render up to `max` lines of `body` (word-wrapped), with a footer stating
/// how many lines were hidden.
fn write_capped_lines<W: Write>(w: &mut W, body: &str, max: usize) -> io::Result<()> {
    let total = body.lines().count();
    let shown: Vec<&str> = body.lines().take(max).collect();
    write_wrapped_lines(
        w,
        &shown.join("\n"),
        &format!("{BG_DIM}      {DIM}"),
        &format!("      {BG_DIM}{DIM}"),
        MAX_LINE_WIDTH,
        true,
    )
    .map_err(|_| io::Error::other("wrap failed"))?;
    if total > shown.len() {
        writeln!(
            w,
            "{BG_DIM}      {DIM}┊ +{} more lines{FILL_EOL}{RESET}",
            total - shown.len()
        )?;
    }
    Ok(())
}

/// Render a `read` result: `(N lines)` meta line, then up to
/// [`MAX_READ_LINES`] content lines and a hidden-lines footer.
fn write_read_body<W: Write>(w: &mut W, body: &str) -> io::Result<()> {
    let mut lines = body.lines();
    let first = lines.next().unwrap_or("");
    let is_meta = first.starts_with('(') && first.contains(" lines") && first.ends_with(')');

    let (meta, content) = if is_meta {
        (Some(first), lines.collect::<Vec<_>>())
    } else {
        (
            None,
            std::iter::once(first).chain(lines).collect::<Vec<_>>(),
        )
    };

    if let Some(meta) = meta {
        band_line(w, meta, DIM)?;
    }

    let total = content.len();
    let shown: Vec<&str> = content.iter().take(MAX_READ_LINES).copied().collect();
    for line in &shown {
        band_line(w, &truncate_ellipsis(line, MAX_LINE_WIDTH), DIM)?;
    }
    if total > shown.len() {
        writeln!(
            w,
            "{BG_DIM}      {DIM}┊ +{} more lines{FILL_EOL}{RESET}",
            total - shown.len()
        )?;
    }
    Ok(())
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
            if !last.content.is_empty() {
                last.content.push(' ');
            }
            last.content.push_str(line.trim());
        }
    }

    if entries.is_empty() || entries.iter().any(|e| e.url.is_empty()) {
        return None;
    }
    Some(entries)
}

/// Render parsed search entries as a link-styled list (matching the markdown
/// renderer's link treatment: cyan text, dim URL).
fn write_search_entries<W: Write>(w: &mut W, entries: &[SearchEntry]) -> io::Result<()> {
    let total = entries.len();
    for entry in entries.iter().take(MAX_SEARCH_RESULTS) {
        writeln!(
            w,
            "{BG_DIM}      {GRAY}•{RESET}{BG_DIM} {CYAN}{}{RESET}{BG_DIM} {DIM}({}){FILL_EOL}{RESET}",
            entry.title, entry.url
        )?;
        let snippet = entry.content.lines().next().unwrap_or("");
        if !snippet.is_empty() {
            band_line(w, &format!("  {}", truncate_ellipsis(snippet, 100)), DIM)?;
        }
    }
    if total > MAX_SEARCH_RESULTS {
        writeln!(
            w,
            "{BG_DIM}      {DIM}┊ +{} more results{FILL_EOL}{RESET}",
            total - MAX_SEARCH_RESULTS
        )?;
    }
    Ok(())
}

/// Produce a one-line summary for listing tools (ls, grep, glob).
pub fn summarize_listing_result(result: &str, tool_name: &str) -> String {
    if result.starts_with("Error:") || result.starts_with("No ") || result == "Directory is empty" {
        return result.to_string();
    }

    let lines: Vec<&str> = result.lines().collect();
    let count = lines.len();
    let label = match tool_name {
        "ls" => "entries",
        "grep" => "matches",
        "glob" => "files",
        _ => "results",
    };

    const PREVIEW: usize = 3;
    if count <= PREVIEW {
        format!("{count} {label} — {result}")
    } else {
        let preview: Vec<&str> = lines.iter().take(PREVIEW).copied().collect();
        format!(
            "{count} {label} — {} ... ({} more)",
            preview.join(", "),
            count - PREVIEW
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(s: &str) -> String {
        let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        re.replace_all(s, "").to_string()
    }

    fn render(tool: &str, body: &str, is_error: bool, ms: Option<u64>) -> String {
        let mut buf = Vec::new();
        write_tool_result(&mut buf, tool, body, is_error, ms).unwrap();
        String::from_utf8(buf).unwrap()
    }

    // ── Header ──

    #[test]
    fn success_header_shows_check_icon_and_duration() {
        let out = render("read", "(1 lines)\nhi", false, Some(42));
        assert!(out.contains('✓'));
        assert!(out.contains("read"));
        assert!(out.contains("42ms"));
        assert!(!out.contains('✗'));
    }

    #[test]
    fn error_header_shows_cross_icon() {
        let out = render("run", "Error: boom", true, Some(1500));
        assert!(out.contains('✗'));
        assert!(out.contains(RED));
        assert!(out.contains("1.5s"));
    }

    #[test]
    fn history_omits_duration() {
        let out = render("ls", "a\nb", false, None);
        assert!(!out.contains('·'), "no duration separator: {out:?}");
        assert!(out.contains("ls"));
    }

    #[test]
    fn empty_body_is_header_only() {
        let out = render("run", "", false, Some(5));
        assert_eq!(out.lines().count(), 1);
    }

    // ── Error / denied bodies ──

    #[test]
    fn error_body_is_single_red_line_truncated() {
        let long = format!("Error: {}", "x".repeat(300));
        let out = render("run", &long, true, None);
        assert_eq!(out.lines().count(), 2); // header + one body line
        assert!(out.contains(RED));
        assert!(out.contains('…'));
        assert!(!strip_ansi(&out).contains(&"x".repeat(200)));
    }

    #[test]
    fn denied_tool_renders_orange_single_line() {
        let out = render(
            "run",
            "[Tool denied] The user denied the 'run' tool call with arguments: cmd=\"rm\"",
            false,
            None,
        );
        assert!(out.contains(ORANGE));
        assert_eq!(out.lines().count(), 2);
    }

    // ── Listing tools ──

    #[test]
    fn listing_summary_line() {
        let out = render("ls", "Cargo.toml\nsrc\ntarget\nx\ny", false, None);
        let plain = strip_ansi(&out);
        assert!(plain.contains("5 entries — Cargo.toml, src, target ... (2 more)"));
        assert_eq!(plain.lines().count(), 2);
    }

    #[test]
    fn listing_error_passes_through() {
        let out = render("grep", "Error: bad pattern", true, None);
        assert!(strip_ansi(&out).contains("Error: bad pattern"));
    }

    // ── read ──

    #[test]
    fn read_shows_meta_then_capped_preview() {
        let content: Vec<String> = (1..=20).map(|i| format!("line{i}")).collect();
        let body = format!("(20 lines)\n{}", content.join("\n"));
        let out = render("read", &body, false, None);
        let plain = strip_ansi(&out);
        assert!(plain.contains("(20 lines)"));
        assert!(plain.contains("line1"));
        assert!(plain.contains("line3"));
        assert!(!plain.contains("line4"));
        assert!(plain.contains("┊ +17 more lines"));
    }

    #[test]
    fn read_without_meta_line_renders_all_as_content() {
        let out = render("read", "just one line", false, None);
        let plain = strip_ansi(&out);
        assert!(plain.contains("just one line"));
        assert!(!plain.contains("┊"));
    }

    // ── run / default capping ──

    #[test]
    fn run_output_capped_with_footer() {
        let content: Vec<String> = (1..=50).map(|i| format!("out{i}")).collect();
        let out = render("run", &content.join("\n"), false, Some(2300));
        let plain = strip_ansi(&out);
        assert!(plain.contains("out1"));
        assert!(plain.contains("out10"));
        assert!(!plain.contains("out11"));
        assert!(plain.contains("┊ +40 more lines"));
        assert!(out.contains("2.3s"));
    }

    #[test]
    fn short_output_not_capped() {
        let out = render("run", "one\ntwo", false, None);
        assert!(!out.contains("┊"));
        let plain = strip_ansi(&out);
        assert!(plain.contains("one"));
        assert!(plain.contains("two"));
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
        let plain = strip_ansi(&out);
        assert!(plain.contains("• Title 1"));
        assert!(plain.contains("(https://x.dev/1)"));
        assert!(plain.contains("Snippet 1 text"));
        assert!(out.contains(CYAN), "titles should be link-colored");
        assert!(!out.contains("┊"), "3 entries fit under the cap");
    }

    #[test]
    fn web_search_caps_results_with_footer() {
        let out = render("web_search", &search_body(8), false, None);
        let plain = strip_ansi(&out);
        assert!(plain.contains("Title 1"));
        assert!(plain.contains("Title 5"));
        assert!(!plain.contains("Title 6"));
        assert!(plain.contains("┊ +3 more results"));
    }

    #[test]
    fn web_search_unparseable_falls_back_to_raw() {
        let out = render("web_search", "No results found.", false, None);
        let plain = strip_ansi(&out);
        assert!(plain.contains("No results found."));
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
        assert!(strip_ansi(&out).contains("see section one"));
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
        assert_eq!(out.lines().count(), 2);
        assert!(strip_ansi(&out).contains("User answered the question"));
    }

    #[test]
    fn debug_web_search_parse() {
        let body = "[1] Title 1\n    URL: https://x.dev/1\n    Snippet 1 text\n";
        match super::parse_web_search(body) {
            Some(entries) => {
                for e in &entries {
                    println!(
                        "ENTRY title={:?} url={:?} content={:?}",
                        e.title, e.url, e.content
                    );
                }
            }
            None => println!("PARSE RETURNED NONE"),
        }
    }

    #[test]
    fn debug_web_search_full_render() {
        let body: String = (1..=3)
            .map(|i| format!("[{i}] Title {i}\n    URL: https://x.dev/{i}\n    Snippet {i} text\n"))
            .collect::<Vec<_>>()
            .join("\n");
        println!("BODY: {body:?}");
        let mut buf = Vec::new();
        super::write_tool_result(&mut buf, "web_search", &body, false, None).unwrap();
        for l in String::from_utf8(buf).unwrap().lines() {
            println!("OUT: {l:?}");
        }
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
            "3 matches — a\nb\nc"
        );
        assert_eq!(summarize_listing_result("a", "ls"), "1 entries — a");
    }

    #[test]
    fn summarize_error_passthrough() {
        assert_eq!(summarize_listing_result("Error: nope", "ls"), "Error: nope");
    }

    #[test]
    fn truncate_ellipsis_is_char_boundary_safe() {
        let s = "żółw".repeat(50);
        let t = truncate_ellipsis(&s, 10);
        assert!(t.ends_with('…'));
        assert!(t.is_char_boundary(0));
    }
}
