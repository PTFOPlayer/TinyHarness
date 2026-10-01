//! A conservative, dependency-free markdown renderer for terminal output.
//!
//! Model responses are streamed token-by-token, which makes full re-rendering
//! impossible — styling markers (`` ` ``, `**`, fences) arrive split across
//! chunks. This module solves that by buffering **lines**: text is emitted as
//! soon as a newline finalizes it, so output still feels live while constructs
//! like fenced code blocks, headings, and lists are styled correctly.
//!
//! Design rules:
//! - **Never lose text.** If a construct can't be completed (unmatched `**`,
//!   unterminated link, etc.) the raw characters are emitted untouched.
//! - **Styling can't leak.** If the model emits raw ANSI sequences in its
//!   prose, each affected line is closed with `RESET` so stray styling never
//!   bleeds into subsequent lines or UI output.
//! - Single-`*` italics and `_underscores_` are deliberately *not* styled —
//!   they appear too often in code/glob/math contexts to detect safely.
//! - ANSI is only emitted for constructs that are actually detected, so plain
//!   prose streams through with zero escape-sequence noise.
//!
//! [`MarkdownStream`] is the incremental (live) API; [`render_markdown`] is the
//! one-shot API used for re-printing saved history.

use std::io::{self, Write};

use crate::style::*;

/// Incremental markdown renderer for streamed content.
///
/// Feed deltas with [`push`](Self::push); call [`finish`](Self::finish) when
/// the generation ends (including on interrupt/error) to flush the trailing
/// partial line.
#[derive(Debug, Default)]
pub struct MarkdownStream {
    core: MdCore,
    /// Whether the initial [`ASSISTANT_TEXT`] style has been emitted.
    started: bool,
}

impl MarkdownStream {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consume a streamed content delta, emitting finished lines.
    pub fn push<W: Write>(&mut self, w: &mut W, delta: &str) -> io::Result<()> {
        if delta.is_empty() {
            return Ok(());
        }
        if !self.started {
            // Reset any styling left over from the spinner / thinking block /
            // model-emitted escapes before the first visible content.
            w.write_all(ASSISTANT_TEXT.as_bytes())?;
            self.started = true;
        }
        for ch in delta.chars() {
            if ch == '\n' {
                let line = std::mem::take(&mut self.core.line_buf);
                let styled = self.core.finalize_line(&line);
                w.write_all(styled.as_bytes())?;
                w.write_all(b"\n")?;
            } else if ch != '\r' {
                self.core.line_buf.push(ch);
            }
        }
        Ok(())
    }

    /// Flush the trailing partial line (if any) without adding a newline.
    ///
    /// Note: the instance is *not* reusable after `finish` in practice —
    /// `started` stays `true`, so no redundant second reset is emitted. Use a
    /// fresh [`MarkdownStream`] per generation.
    pub fn finish<W: Write>(&mut self, w: &mut W) -> io::Result<()> {
        if !self.core.line_buf.is_empty() {
            let line = std::mem::take(&mut self.core.line_buf);
            let styled = self.core.finalize_line(&line);
            w.write_all(styled.as_bytes())?;
        }
        self.core.fence = None;
        Ok(())
    }
}

/// One-shot renderer for complete texts (e.g. re-printing a saved session).
pub fn render_markdown(text: &str) -> String {
    let mut core = MdCore::default();
    let mut out = String::with_capacity(text.len());
    for line in text.split('\n') {
        out.push_str(&core.finalize_line(line.trim_end_matches('\r')));
        out.push('\n');
    }
    // `split` on a trailing '\n' yields a final empty fragment that must not
    // become an extra blank line.
    if text.ends_with('\n') && out.ends_with("\n\n") {
        out.truncate(out.len() - 1);
    }
    out
}

#[derive(Debug, Default)]
struct MdCore {
    line_buf: String,
    /// Opening fence marker of an open code block (``` or ~~~).
    fence: Option<String>,
}

impl MdCore {
    /// Style one complete line. May return an empty string (consumed lines,
    /// e.g. code-fence delimiters).
    ///
    /// Wraps [`Self::style_line`] with ANSI containment: if the model emitted raw
    /// escape sequences in its prose, they must not leak styling into subsequent
    /// lines. Paragraph and list-item tails don't otherwise end with `RESET`, so
    /// append one when the raw line contained ESC and the styled output doesn't
    /// already end in a reset (idempotent, so this is always safe).
    fn finalize_line(&mut self, line: &str) -> String {
        let styled = self.style_line(line);
        if line.contains('\x1b') && !styled.ends_with(RESET) {
            styled + RESET
        } else {
            styled
        }
    }

    /// Core line styler — see [`Self::finalize_line`].
    fn style_line(&mut self, line: &str) -> String {
        let trimmed = line.trim();

        // ── Inside a fenced code block ──
        if let Some(marker) = &self.fence {
            if trimmed.starts_with(marker.as_str()) {
                // Closing fence: hide it.
                self.fence = None;
                return String::new();
            }
            let mut out = String::with_capacity(line.len() + 16);
            out.push_str(GRAY);
            out.push_str("    ");
            out.push_str(line.trim_start());
            out.push_str(RESET);
            return out;
        }
        if let Some(lang) = fence_opening(trimmed) {
            self.fence = Some(lang.0);
            let mut out = String::new();
            if !lang.1.is_empty() {
                out.push_str(DIM);
                out.push_str(ITALIC);
                out.push_str(&lang.1);
                out.push_str(RESET);
            }
            return out;
        }

        // ── Block constructs (matched on trimmed text) ──
        if let Some((level, text)) = heading(trimmed) {
            let color = if level <= 2 { TITLE_COLOR } else { GRAY };
            return format!("{BOLD}{color}{}{RESET}", inline(text));
        }
        if is_rule(trimmed) {
            return format!("{DIM}{}{RESET}", "─".repeat(40));
        }
        if let Some(rest) = trimmed.strip_prefix("> ") {
            return format!("{DIM}│ {RESET}{GRAY}{}{RESET}", inline(rest));
        }
        if trimmed == ">" {
            return format!("{DIM}│{RESET}");
        }
        if let Some((indent, marker, rest)) = list_item(line) {
            let pad = " ".repeat(indent);
            return format!("{pad}{GRAY}{marker}{RESET} {}", inline(rest));
        }

        // ── Plain paragraph line ──
        inline(line)
    }
}

/// Returns `Some((marker, language))` if the line opens a code fence.
fn fence_opening(trimmed: &str) -> Option<(String, String)> {
    for marker in ["```", "~~~"] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            return Some((marker.to_string(), rest.trim().to_string()));
        }
    }
    None
}

/// `# Heading` .. `###### Heading` → `(level, text)`.
fn heading(trimmed: &str) -> Option<(u8, &str)> {
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes)
        && let Some(rest) = trimmed[hashes..].strip_prefix(' ')
    {
        let text = rest.trim_end();
        if !text.is_empty() {
            return Some((hashes as u8, text));
        }
    }
    None
}

/// `---`, `***`, `___` (3+) → horizontal rule. Allocation-free: bails out at
/// the first character that can't belong to a rule.
fn is_rule(trimmed: &str) -> bool {
    let mut first: Option<char> = None;
    let mut count = 0usize;
    for c in trimmed.chars() {
        match first {
            None => {
                if !matches!(c, '-' | '*' | '_') {
                    return false;
                }
                first = Some(c);
                count = 1;
            }
            Some(f) if c == f => count += 1,
            Some(_) if c != ' ' => return false,
            _ => {}
        }
    }
    count >= 3
}

/// Detect `- item`, `* item`, `+ item` or `1. item`. Returns
/// `(indent width, display marker, item text)`. `-`/`*` render as `•`.
fn list_item(line: &str) -> Option<(usize, String, &str)> {
    let indent = line.len() - line.trim_start().len();
    let rest = line.trim_start();
    if let Some(after) = rest.strip_prefix(['-', '+'])
        && let Some(text) = after.strip_prefix(' ')
    {
        let text = text.trim_start();
        if !text.is_empty() {
            return Some((indent, "•".to_string(), text));
        }
    }
    if let Some(after) = rest.strip_prefix('*')
        && let Some(text) = after.strip_prefix(' ')
    {
        let text = text.trim_start();
        // Guard: `** bold **`-style lines are not lists.
        if !text.is_empty() && !text.starts_with('*') {
            return Some((indent, "•".to_string(), text));
        }
    }
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty()
        && let Some(after) = rest[digits.len()..].strip_prefix(". ")
    {
        let text = after.trim_start();
        if !text.is_empty() {
            return Some((indent, format!("{digits}."), text));
        }
    }
    None
}

/// Inline styling: `**bold**`, `` `code` ``, `[text](url)`.
///
/// Unmatched markers are emitted verbatim — text is never dropped.
fn inline(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap();
        match c {
            '*' => {
                if let Some(inner) = s[i..].strip_prefix("**") {
                    match inner.find("**") {
                        Some(end) if end > 0 => {
                            out.push_str(BOLD);
                            out.push_str(&inner[..end]);
                            out.push_str(RESET);
                            // 2 (opening **) + content + 2 (closing **)
                            i += end + 4;
                            continue;
                        }
                        _ => {}
                    }
                }
                out.push('*');
                i += 1;
            }
            '`' => {
                if let Some(end) = s[i + 1..].find('`')
                    && end > 0
                {
                    out.push_str(GRAY);
                    out.push('`');
                    out.push_str(&s[i + 1..i + 1 + end]);
                    out.push('`');
                    out.push_str(RESET);
                    i += end + 2;
                    continue;
                }
                out.push('`');
                i += 1;
            }
            '[' | '!' => {
                // [text](url) or ![alt](url)
                let is_image = c == '!';
                let open = if is_image { i + 1 } else { i };
                if s.get(open..).and_then(|r| r.strip_prefix('[')).is_some() {
                    let after_bracket = &s[open + 1..];
                    if let Some(text_end) = after_bracket.find("](")
                        && text_end > 0
                    {
                        let tail = &after_bracket[text_end + 2..];
                        if let Some(url_end) = tail.find(')') {
                            let text = &after_bracket[..text_end];
                            let url = &tail[..url_end];
                            if is_image {
                                out.push('!');
                            }
                            out.push_str(CYAN);
                            out.push_str(text);
                            out.push_str(RESET);
                            out.push_str(DIM);
                            out.push('(');
                            out.push_str(url);
                            out.push(')');
                            out.push_str(RESET);
                            i = open + 1 + text_end + 2 + url_end + 1;
                            continue;
                        }
                    }
                }
                out.push(c);
                i += c.len_utf8();
            }
            _ => {
                out.push(c);
                i += c.len_utf8();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(s: &str) -> String {
        let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        re.replace_all(s, "").to_string()
    }

    // ── Block constructs ──

    #[test]
    fn heading_is_bold_and_styled() {
        let out = render_markdown("# Title\n");
        assert!(out.contains(BOLD));
        assert!(out.contains("Title"));
        assert_eq!(strip_ansi(&out).trim_end(), "Title");
        assert!(!strip_ansi(&out).contains('#'));
    }

    #[test]
    fn h3_heading_uses_dim_color_variant() {
        let out = render_markdown("### Sub\n");
        assert!(out.contains(GRAY));
        assert_eq!(strip_ansi(&out).trim_end(), "Sub");
    }

    #[test]
    fn heading_without_space_not_styled() {
        // `#hashtag` is not a heading — must pass through untouched.
        let out = render_markdown("#hashtag\n");
        assert_eq!(out, "#hashtag\n");
    }

    #[test]
    fn bullet_points_rendered() {
        let out = render_markdown("- one\n* two\n");
        assert!(out.contains('•'));
        assert_eq!(strip_ansi(&out), "• one\n• two\n");
    }

    #[test]
    fn nested_list_keeps_indent() {
        let out = render_markdown("  - child\n");
        assert!(out.starts_with("  "));
        assert_eq!(strip_ansi(&out), "  • child\n");
    }

    #[test]
    fn numbered_list_keeps_number() {
        let out = render_markdown("3. third\n");
        assert_eq!(strip_ansi(&out), "3. third\n");
    }

    #[test]
    fn bold_stars_line_is_not_a_list() {
        let out = render_markdown("**Important** note\n");
        assert!(out.contains(BOLD));
        assert_eq!(strip_ansi(&out), "Important note\n");
    }

    #[test]
    fn code_fence_block_styled_and_markers_hidden() {
        let out = render_markdown("```rust\nfn main() {}\n```\n");
        let plain = strip_ansi(&out);
        assert!(!plain.contains("```"));
        assert!(plain.contains("    fn main() {}"));
        assert!(out.contains(GRAY));
    }

    #[test]
    fn code_fence_shows_language_tag_dim() {
        let out = render_markdown("```rust\nx\n```\n");
        assert!(out.contains(ITALIC));
        assert!(out.contains("rust"));
    }

    #[test]
    fn code_fence_content_never_styled() {
        // Markdown inside a code block must not be interpreted.
        let out = render_markdown("```\n# not a heading\n**not bold**\n```\n");
        let plain = strip_ansi(&out);
        assert!(plain.contains("# not a heading"));
        assert!(plain.contains("**not bold**"));
        assert!(!out.contains(BOLD));
    }

    #[test]
    fn unclosed_fence_streams_content_as_code() {
        let mut core = MdCore::default();
        assert_eq!(core.finalize_line("```"), "");
        assert!(core.fence.is_some());
        let line = core.finalize_line("code here");
        assert!(line.contains(GRAY));
        assert!(line.contains("code here"));
    }

    #[test]
    fn blockquote_renders_bar() {
        let out = render_markdown("> quoted text\n");
        assert!(out.contains('│'));
        assert_eq!(strip_ansi(&out), "│ quoted text\n");
    }

    #[test]
    fn horizontal_rule_converted() {
        let out = render_markdown("---\n");
        assert!(out.contains("─"));
        assert!(!strip_ansi(&out).contains("---"));
    }

    #[test]
    fn dashes_inside_text_untouched() {
        let out = render_markdown("a - - b\n");
        assert_eq!(out, "a - - b\n");
    }

    // ── Inline constructs ──

    #[test]
    fn inline_bold() {
        let out = render_markdown("this is **bold** text\n");
        assert!(out.contains(BOLD));
        assert_eq!(strip_ansi(&out), "this is bold text\n");
    }

    #[test]
    fn unmatched_bold_marker_preserved() {
        let out = render_markdown("2 * 3 ** 4\n");
        assert_eq!(strip_ansi(&out), "2 * 3 ** 4\n");
    }

    #[test]
    fn inline_code_span() {
        let out = render_markdown("call `foo()` now\n");
        assert!(out.contains(GRAY));
        assert_eq!(strip_ansi(&out), "call `foo()` now\n");
    }

    #[test]
    fn unmatched_backtick_preserved() {
        let out = render_markdown("one ` tick\n");
        assert_eq!(strip_ansi(&out), "one ` tick\n");
    }

    #[test]
    fn link_shows_text_and_url() {
        let out = render_markdown("see [docs](https://x.dev) please\n");
        assert!(out.contains(CYAN));
        assert!(out.contains("docs"));
        assert!(out.contains("https://x.dev"));
    }

    #[test]
    fn broken_link_left_raw() {
        let out = render_markdown("a [b c\n");
        assert_eq!(strip_ansi(&out), "a [b c\n");
    }

    #[test]
    fn plain_text_gets_no_ansi() {
        let out = render_markdown("just a normal sentence\n");
        assert_eq!(out, "just a normal sentence\n");
    }

    #[test]
    fn unicode_safe() {
        let out = render_markdown("żółw **gruby** 🐢\n");
        assert_eq!(strip_ansi(&out), "żółw gruby 🐢\n");
    }

    // ── Raw ANSI containment (model-emitted escapes) ──

    #[test]
    fn model_emitted_ansi_is_contained_on_plain_line() {
        let red = "\x1b[31m";
        let out = render_markdown(&format!("hello {red}red world\n"));
        // Styling is neutralized before the next line begins.
        assert!(out.ends_with(&format!("{RESET}\n")));
        assert!(
            out.contains(red),
            "raw sequence must pass through, not be dropped"
        );
        assert_eq!(strip_ansi(&out), "hello red world\n");
    }

    #[test]
    fn model_emitted_ansi_does_not_leak_to_next_line() {
        let red = "\x1b[31m";
        let out = render_markdown(&format!("line one {red}colored\nline two\n"));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        // The second line carries no residual color: it is plain text with
        // zero escape sequences of its own.
        assert_eq!(lines[1], "line two");
    }

    #[test]
    fn styled_line_ending_in_reset_is_not_double_reset() {
        // A construct-styled line already ends with RESET — containment must
        // be idempotent (no doubled escape).
        let out = render_markdown("already **styled**\n");
        let suffix = format!("{BOLD}styled{RESET}");
        assert!(out.contains(&suffix));
        // Exactly one RESET at end-of-line.
        let after_suffix = &out[out.find(&suffix).unwrap() + suffix.len()..];
        assert!(!after_suffix.starts_with(RESET));
    }

    #[test]
    fn fence_content_with_ansi_is_contained() {
        let red = "\x1b[31m";
        let out = render_markdown(&format!("```\n{red}colored code\n```\n"));
        // Fence delimiters are hidden (empty lines); only the content line is
        // non-empty. Fence styling already ends with RESET, so containment is
        // idempotent and no raw escape can leak past it.
        let non_empty: Vec<&str> = out.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(non_empty.len(), 1);
        assert!(non_empty[0].contains("colored code"));
        assert!(non_empty[0].ends_with(RESET));
    }

    #[test]
    fn stream_containment_matches_oneshot() {
        // The streaming path goes through the same finalize_line wrapper, so
        // its containment behavior must match the one-shot renderer.
        let mut md = MarkdownStream::new();
        let mut buf = Vec::new();
        md.push(&mut buf, "a \x1b[32mb\n").unwrap();
        md.finish(&mut buf).unwrap();
        let streamed = String::from_utf8(buf).unwrap();
        let oneshot = render_markdown("a \x1b[32mb\n");
        let streamed_body = streamed.strip_prefix(ASSISTANT_TEXT).unwrap_or(&streamed);
        assert_eq!(streamed_body, oneshot);
    }

    // ── Horizontal-rule edge cases ──

    #[test]
    fn two_dashes_is_not_a_rule() {
        let out = render_markdown("--\n");
        assert_eq!(strip_ansi(&out), "--\n");
    }

    #[test]
    fn mixed_rule_chars_not_a_rule() {
        let out = render_markdown("-*- \n");
        assert_eq!(strip_ansi(&out), "-*- \n");
    }

    #[test]
    fn spaced_rule_is_a_rule() {
        let out = render_markdown("- - -\n");
        assert!(strip_ansi(&out).contains('─'));
    }

    #[test]
    fn long_underscore_rule_converted() {
        let out = render_markdown("_____\n");
        assert!(strip_ansi(&out).contains('─'));
    }

    // ── Streaming behavior ──

    #[test]
    fn stream_emits_line_only_on_newline() {
        let mut md = MarkdownStream::new();
        let mut buf = Vec::new();
        md.push(&mut buf, "# Hel").unwrap();
        // No newline yet: nothing visible except the initial reset.
        assert_eq!(String::from_utf8_lossy(&buf), ASSISTANT_TEXT);
        md.push(&mut buf, "lo world\nnext\n").unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("Hello world"));
        assert!(s.contains("next"));
    }

    #[test]
    fn stream_marker_split_across_deltas_is_styled() {
        let mut md = MarkdownStream::new();
        let mut buf = Vec::new();
        md.push(&mut buf, "**bo").unwrap();
        md.push(&mut buf, "ld**\n").unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains(BOLD));
        assert_eq!(strip_ansi(&s[ASSISTANT_TEXT.len()..]), "bold\n");
    }

    #[test]
    fn stream_fence_spans_multiple_chunks() {
        let mut md = MarkdownStream::new();
        let mut buf = Vec::new();
        md.push(&mut buf, "```\n").unwrap();
        md.push(&mut buf, "let x = 1;\n").unwrap();
        md.push(&mut buf, "```\n").unwrap();
        let s = String::from_utf8(buf).unwrap();
        let plain = strip_ansi(&s);
        assert!(!plain.contains("```"));
        assert!(plain.contains("let x = 1;"));
    }

    #[test]
    fn finish_flushes_partial_line_without_newline() {
        let mut md = MarkdownStream::new();
        let mut buf = Vec::new();
        md.push(&mut buf, "complete\npartial tail").unwrap();
        md.finish(&mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.ends_with("partial tail"));
        assert!(!s.ends_with("partial tail\n"));
    }

    #[test]
    fn crlf_input_is_normalized() {
        let mut md = MarkdownStream::new();
        let mut buf = Vec::new();
        md.push(&mut buf, "line one\r\nline two\r\n").unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, format!("{ASSISTANT_TEXT}line one\nline two\n"));
    }

    #[test]
    fn empty_push_is_noop() {
        let mut md = MarkdownStream::new();
        let mut buf = Vec::new();
        md.push(&mut buf, "").unwrap();
        md.finish(&mut buf).unwrap();
        assert!(buf.is_empty());
    }
}
