// ── Conversation widget ─────────────────────────────────────────────────────
//
// Displays the conversation history in a scrollable pane with
// color-coded messages, tool call blocks, and thinking chains.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui::cell::{Cell, Color, Style};
use crate::tui::event::{Event, Key, KeyEvent};
use crate::tui::layout::Rect;
use crate::tui::screen::Screen;
use crate::tui::widget::{Action, Widget, styles, truncate_str_width};

/// A single line in the conversation display.
#[derive(Clone, Debug)]
pub enum ConversationLine {
    /// A user message.
    User { text: String },
    /// An assistant message.
    Assistant { text: String },
    /// A tool call header (e.g., "── Tool: read ──").
    ToolCall { name: String, args_summary: String },
    /// A tool result.
    ToolResult {
        name: String,
        content: String,
        is_error: bool,
    },
    /// A system message.
    System { text: String },
    /// Thinking/reasoning chain content.
    Thinking { text: String },
    /// A horizontal separator line.
    Separator,
    /// A confirmation prompt for a tool call, awaiting user y/n/a response.
    /// `diff_preview` contains a plain-text unified diff to show (for edit/write).
    ConfirmPrompt {
        name: String,
        args_summary: String,
        diff_preview: Option<String>,
    },
    /// A question prompt from the question tool with numbered answers.
    Question {
        question: String,
        answers: Vec<String>,
    },
}

/// Context warning level for the banner at the top of the conversation.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ContextWarningLevel {
    #[default]
    None,
    /// 70%+ of context window used.
    Warning(f64),
    /// 90%+ of context window used.
    Critical(f64),
}

/// Search state for the conversation widget.
#[derive(Clone, Debug, Default)]
struct SearchState {
    /// Whether search mode is active (Ctrl+F toggled).
    active: bool,
    /// The current search query.
    query: String,
    /// Cursor position within the search query (byte offset).
    cursor: usize,
    /// Visual row offsets where matches start. Computed on each render.
    match_rows: Vec<usize>,
    /// Index of the currently highlighted match (for navigation).
    current_match: usize,
}

/// Scrollable conversation pane.
pub struct ConversationWidget {
    lines: Vec<ConversationLine>,
    /// Scroll offset in **visual row units** (not conversation line units).
    scroll_offset: usize,
    auto_scroll: bool,
    /// Context warning level (shown as a banner at the top).
    context_warning: ContextWarningLevel,
    /// Search state (Ctrl+F to toggle).
    search: SearchState,
}

impl ConversationWidget {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            scroll_offset: 0,
            auto_scroll: true,
            context_warning: ContextWarningLevel::None,
            search: SearchState::default(),
        }
    }

    /// Add a line to the conversation.
    pub fn push(&mut self, line: ConversationLine) {
        self.lines.push(line);
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Add multiple lines at once.
    pub fn extend(&mut self, lines: Vec<ConversationLine>) {
        self.lines.extend(lines);
        if self.auto_scroll {
            self.scroll_to_bottom();
        }
    }

    /// Clear all conversation lines.
    pub fn clear(&mut self) {
        self.lines.clear();
        self.scroll_offset = 0;
    }

    /// Get a mutable reference to the last line, if any.
    pub fn last_mut(&mut self) -> Option<&mut ConversationLine> {
        self.lines.last_mut()
    }

    /// Check if the last line is an assistant message.
    pub fn last_is_assistant(&self) -> bool {
        matches!(self.lines.last(), Some(ConversationLine::Assistant { .. }))
    }

    /// Set the context warning level.
    pub fn set_context_warning(&mut self, level: ContextWarningLevel) {
        self.context_warning = level;
    }

    /// Get the current context warning level.
    pub fn context_warning(&self) -> &ContextWarningLevel {
        &self.context_warning
    }

    /// Toggle search mode on/off.
    pub fn toggle_search(&mut self) {
        if self.search.active {
            self.close_search();
        } else {
            self.search.active = true;
            self.search.query.clear();
            self.search.cursor = 0;
            self.search.match_rows.clear();
            self.search.current_match = 0;
        }
    }

    /// Close search mode and clear search state.
    pub fn close_search(&mut self) {
        self.search.active = false;
        self.search.query.clear();
        self.search.cursor = 0;
        self.search.match_rows.clear();
        self.search.current_match = 0;
    }

    /// Check if search mode is active.
    pub fn is_search_active(&self) -> bool {
        self.search.active
    }

    /// Type a character into the search query.
    fn search_type_char(&mut self, ch: char) {
        self.search.query.insert(self.search.cursor, ch);
        self.search.cursor += ch.len_utf8();
        self.recompute_search_matches();
        self.search.current_match = 0;
    }

    /// Delete the character before the cursor in the search query.
    fn search_backspace(&mut self) {
        if self.search.cursor > 0 {
            // Find the previous char boundary
            let prev = self.search.query[..self.search.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.search.query.drain(prev..self.search.cursor);
            self.search.cursor = prev;
            self.recompute_search_matches();
            self.search.current_match = 0;
        }
    }

    /// Move cursor left in the search query.
    fn search_cursor_left(&mut self) {
        if self.search.cursor > 0 {
            let prev = self.search.query[..self.search.cursor]
                .char_indices()
                .next_back()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.search.cursor = prev;
        }
    }

    /// Move cursor right in the search query.
    fn search_cursor_right(&mut self) {
        if self.search.cursor < self.search.query.len() {
            let next = self.search.query[self.search.cursor..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.search.cursor + i)
                .unwrap_or(self.search.query.len());
            self.search.cursor = next;
        }
    }

    /// Navigate to the next search match.
    fn search_next(&mut self) {
        if !self.search.match_rows.is_empty() {
            self.search.current_match =
                (self.search.current_match + 1) % self.search.match_rows.len();
            let target_row = self.search.match_rows[self.search.current_match];
            self.scroll_offset = target_row;
            self.auto_scroll = false;
        }
    }

    /// Navigate to the previous search match.
    fn search_prev(&mut self) {
        if !self.search.match_rows.is_empty() {
            if self.search.current_match == 0 {
                self.search.current_match = self.search.match_rows.len() - 1;
            } else {
                self.search.current_match -= 1;
            }
            let target_row = self.search.match_rows[self.search.current_match];
            self.scroll_offset = target_row;
            self.auto_scroll = false;
        }
    }

    /// Recompute the visual row offsets where search matches occur.
    fn recompute_search_matches(&mut self) {
        self.search.match_rows.clear();
        if self.search.query.is_empty() {
            return;
        }
        let query_lower = self.search.query.to_lowercase();
        let mut visual_row = 0usize;
        for line in &self.lines {
            let text = match line {
                ConversationLine::User { text } => text.clone(),
                ConversationLine::Assistant { text } => text.clone(),
                ConversationLine::System { text } => text.clone(),
                ConversationLine::Thinking { text } => text.clone(),
                ConversationLine::ToolResult { content, .. } => content.clone(),
                ConversationLine::ToolCall { name, args_summary } => {
                    format!("{} {}", name, args_summary)
                }
                ConversationLine::ConfirmPrompt {
                    name, args_summary, ..
                } => {
                    format!("{} {}", name, args_summary)
                }
                ConversationLine::Question { question, answers } => {
                    format!("{} {}", question, answers.join(" "))
                }
                ConversationLine::Separator => String::new(),
            };
            if text.to_lowercase().contains(&query_lower) {
                self.search.match_rows.push(visual_row);
            }
            // Advance visual_row — we just need a rough estimate for scrolling
            // Use a simple heuristic: each line is at least 1 visual row
            visual_row += 1;
        }
    }

    /// Calculate how many visual rows a conversation line occupies given the
    /// area width. Uses character-level wrapping to match actual rendering.
    fn line_height(&self, line: &ConversationLine, area_width: u16) -> usize {
        let (text, prefix_len) = match line {
            ConversationLine::User { text } => (text, 7),
            ConversationLine::Assistant { text } => (text, 7),
            ConversationLine::System { text } => (text, 2),
            ConversationLine::Thinking { text } => (text, 13),
            ConversationLine::ToolResult { content, .. } => {
                // For tool results, trim trailing blank lines and limit to
                // a reasonable number of visible lines
                let trimmed = content.trim_end_matches('\n');
                let lines: Vec<&str> = trimmed.lines().collect();
                // Show at most 20 lines for a tool result
                let visible_lines = lines.iter().copied().take(20).collect::<Vec<&str>>();
                let joined = visible_lines.join("\n");
                return self.line_height_for_text(&joined, area_width, 4, 4).max(1);
            }
            ConversationLine::ToolCall { name, args_summary } => {
                if args_summary.is_empty() {
                    return 1;
                }
                let header = format!("  ── {}", name);
                let header_len = header.len();
                return self.line_height_for_text(args_summary, area_width, header_len, header_len);
            }
            ConversationLine::Question { question, answers } => {
                let question_rows = self.line_height_for_text(question, area_width, 4, 4);
                let answer_rows: usize = answers
                    .iter()
                    .map(|a| {
                        let prefix_len = if answers.len() >= 10 { 7 } else { 6 };
                        self.line_height_for_text(a, area_width, prefix_len, prefix_len)
                    })
                    .sum();
                return question_rows + answer_rows;
            }
            ConversationLine::Separator => return 1,
            ConversationLine::ConfirmPrompt { diff_preview, .. } => {
                let mut rows = 1usize;
                if let Some(diff) = diff_preview
                    && !diff.is_empty()
                {
                    for line in diff.lines() {
                        rows += self.line_height_for_text(line, area_width, 4, 4);
                    }
                }
                return rows;
            }
        };

        self.line_height_for_text(text, area_width, prefix_len, prefix_len)
    }

    /// Calculate visual row count for text rendered in `area_width` columns.
    ///
    /// The first line starts at `start_col`; after a wrap or newline the cursor
    /// returns to `wrap_indent`. Uses Unicode display widths.
    fn line_height_for_text(
        &self,
        text: &str,
        area_width: u16,
        start_col: usize,
        wrap_indent: usize,
    ) -> usize {
        if area_width == 0 {
            return 1;
        }
        if text.is_empty() {
            return 1;
        }

        let wrap_col = area_width as usize;
        let mut rows = 1usize;
        let mut col = start_col;

        for ch in text.chars() {
            let w = ch.width().unwrap_or(1);
            if w == 0 {
                // Combining mark doesn't take up visual columns
                continue;
            }
            if ch == '\n' {
                rows += 1;
                col = wrap_indent;
            } else if col + w > wrap_col {
                rows += 1;
                col = wrap_indent + w;
            } else {
                col += w;
            }
        }
        rows
    }

    /// Calculate the total visual height of all conversation lines.
    fn total_visual_height(&self, width: u16) -> usize {
        self.lines.iter().map(|l| self.line_height(l, width)).sum()
    }

    /// Scroll to the bottom of the conversation.
    pub fn scroll_to_bottom(&mut self) {
        // Use a sentinel large value that will be clamped during render.
        // We track whether auto_scroll is active separately.
        self.auto_scroll = true;
    }

    /// Scroll up by `n` visual rows.
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
        self.auto_scroll = false;
    }

    /// Scroll down by `n` visual rows.
    pub fn scroll_down(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(n);
    }

    /// Scroll to the very top.
    pub fn scroll_home(&mut self) {
        self.scroll_offset = 0;
        self.auto_scroll = false;
    }

    /// Render a single conversation line, with clipping support for scroll.
    fn render_line_clipped(
        &self,
        line: &ConversationLine,
        start_row: u16,
        screen: &mut Screen,
        _width: u16,
        area: Rect,
        skip_top: usize,
        rows_available: usize,
    ) {
        let max_row = area.bottom().saturating_sub(1);
        let effective_max_row = (start_row + rows_available as u16)
            .saturating_sub(1)
            .min(max_row);
        // Reserve 1 column for the scrollbar on the right edge
        let wrap_col = area.x + area.width.saturating_sub(1);

        match line {
            ConversationLine::User { text } => {
                let prefix = "  You: ";
                if skip_top == 0 && start_row <= max_row {
                    screen.write_str(
                        start_row,
                        area.x,
                        prefix,
                        Color::GREEN,
                        Color::Default,
                        Style::bold(),
                    );
                    screen.write_str_wrapped_clipped(
                        start_row,
                        area.x + prefix.len() as u16,
                        text,
                        Color::GREEN,
                        Color::Default,
                        Style::default(),
                        area.x,
                        effective_max_row,
                        wrap_col,
                    );
                } else if skip_top > 0 {
                    screen.write_str_wrapped_skip_clipped(
                        start_row,
                        area.x + prefix.len() as u16,
                        text,
                        Color::GREEN,
                        Color::Default,
                        Style::default(),
                        area.x,
                        effective_max_row,
                        wrap_col,
                        skip_top,
                    );
                }
            }
            ConversationLine::Assistant { text } => {
                let prefix = "  AI:  ";
                if skip_top == 0 && start_row <= max_row {
                    screen.write_str(
                        start_row,
                        area.x,
                        prefix,
                        Color::CYAN,
                        Color::Default,
                        Style::bold(),
                    );
                    screen.write_str_wrapped_clipped(
                        start_row,
                        area.x + prefix.len() as u16,
                        text,
                        Color::WHITE,
                        Color::Default,
                        Style::default(),
                        area.x,
                        effective_max_row,
                        wrap_col,
                    );
                } else if skip_top > 0 {
                    screen.write_str_wrapped_skip_clipped(
                        start_row,
                        area.x + prefix.len() as u16,
                        text,
                        Color::WHITE,
                        Color::Default,
                        Style::default(),
                        area.x,
                        effective_max_row,
                        wrap_col,
                        skip_top,
                    );
                }
            }
            ConversationLine::ToolCall { name, args_summary } => {
                let header = format!("  ── {}", name);
                if skip_top == 0 && start_row <= max_row {
                    screen.write_str(
                        start_row,
                        area.x,
                        &header,
                        styles::TOOL_MSG_FG,
                        Color::Default,
                        Style::default(),
                    );
                    if !args_summary.is_empty() {
                        let args_indent = area.x + header.len() as u16;
                        screen.write_str_wrapped_clipped(
                            start_row,
                            args_indent,
                            args_summary,
                            Color::Ansi(96),
                            Color::Default,
                            Style::dim(),
                            args_indent,
                            effective_max_row,
                            wrap_col,
                        );
                    }
                } else if skip_top > 0 && !args_summary.is_empty() {
                    let header = format!("  ── {}", name);
                    let args_indent = area.x + header.len() as u16;
                    screen.write_str_wrapped_skip_clipped(
                        start_row,
                        args_indent,
                        args_summary,
                        Color::Ansi(96),
                        Color::Default,
                        Style::dim(),
                        args_indent,
                        effective_max_row,
                        wrap_col,
                        skip_top,
                    );
                }
            }
            ConversationLine::ToolResult {
                name: _,
                content,
                is_error,
            } => {
                let default_color = if *is_error {
                    Color::RED
                } else {
                    Color::Ansi(252)
                };
                let bg = if *is_error {
                    Color::Ansi(52)
                } else {
                    Color::Default
                };
                // Trim trailing blank lines and limit to 20 visible lines
                let trimmed = content.trim_end_matches('\n');
                let content_lines: Vec<&str> = trimmed.lines().take(20).collect();
                let mut current_row = start_row;
                let max_content_width = (area.width as usize).saturating_sub(5);

                for (i, content_line) in content_lines.iter().enumerate() {
                    if i < skip_top {
                        current_row += 1;
                        continue;
                    }
                    if current_row > max_row {
                        break;
                    }

                    // Detect diff lines and apply appropriate colors with backgrounds
                    let (line_color, line_bg, line_prefix) = if !is_error {
                        if content_line.starts_with("@@") {
                            // Hunk header: @@ -1,3 +1,3 @@
                            (Color::CYAN, Color::Default, "  │ ")
                        } else if content_line.starts_with("---") || content_line.starts_with("+++")
                        {
                            // File header: --- a/file.rs or +++ b/file.rs
                            (Color::WHITE, Color::Default, "  │ ")
                        } else if content_line.starts_with("-") {
                            // Removed line — red text with dark red background
                            (Color::RED, Color::Ansi(52), "  │ ")
                        } else if content_line.starts_with("+") {
                            // Added line — green text with dark green background
                            (Color::GREEN, Color::Ansi(22), "  │ ")
                        } else {
                            (default_color, bg, "  │ ")
                        }
                    } else {
                        (default_color, bg, "  │ ")
                    };

                    let display = if content_line.is_empty() {
                        line_prefix.to_string()
                    } else if content_line.width() > max_content_width {
                        format!(
                            "{}{}…",
                            line_prefix,
                            truncate_str_width(content_line, max_content_width.saturating_sub(1))
                        )
                    } else {
                        format!("{}{}", line_prefix, content_line)
                    };
                    screen.write_str(
                        current_row,
                        area.x,
                        &display,
                        line_color,
                        line_bg,
                        Style::default(),
                    );
                    // Fill background for diff lines and error tool results
                    if line_bg != Color::Default || *is_error {
                        let fill_bg = if line_bg != Color::Default {
                            line_bg
                        } else {
                            bg
                        };
                        let fill_end = area.x + area.width.saturating_sub(1);
                        let end_col = area.x + display.width().min(area.width as usize - 1) as u16;
                        if end_col < fill_end {
                            for c in end_col..fill_end {
                                if let Some(cell) = screen.get_mut(current_row, c) {
                                    cell.bg = fill_bg;
                                }
                            }
                        }
                    }
                    current_row += 1;
                }
                // If content was truncated, show a truncation indicator
                let total_lines = trimmed.lines().count();
                if total_lines > 20 && skip_top == 0 && current_row <= max_row {
                    let truncation = "  │ …";
                    let trunc_bg = if *is_error { bg } else { Color::Default };
                    screen.write_str(
                        current_row,
                        area.x,
                        truncation,
                        Color::Ansi(244),
                        trunc_bg,
                        Style::dim(),
                    );
                    if *is_error {
                        let fill_end = area.x + area.width.saturating_sub(1);
                        for c in (area.x + truncation.len() as u16)..fill_end {
                            if let Some(cell) = screen.get_mut(current_row, c) {
                                cell.bg = bg;
                            }
                        }
                    }
                }
            }
            ConversationLine::System { text } => {
                if skip_top == 0 && start_row <= max_row {
                    screen.write_str(
                        start_row,
                        area.x,
                        "  ",
                        Color::Default,
                        Color::Default,
                        Style::default(),
                    );
                    screen.write_str_wrapped_clipped(
                        start_row,
                        area.x + 2,
                        text,
                        Color::YELLOW,
                        Color::Default,
                        Style::default(),
                        area.x,
                        effective_max_row,
                        wrap_col,
                    );
                } else if skip_top > 0 {
                    screen.write_str_wrapped_skip_clipped(
                        start_row,
                        area.x + 2,
                        text,
                        Color::YELLOW,
                        Color::Default,
                        Style::default(),
                        area.x,
                        effective_max_row,
                        wrap_col,
                        skip_top,
                    );
                }
            }
            ConversationLine::Thinking { text } => {
                let prefix = "  [thinking] ";
                let content_indent = area.x + prefix.len() as u16;
                if skip_top == 0 && start_row <= max_row {
                    screen.write_str(
                        start_row,
                        area.x,
                        prefix,
                        styles::THINKING_FG,
                        Color::Default,
                        Style::dim(),
                    );
                    let display_text = if text.is_empty() {
                        "⋯".to_string()
                    } else {
                        text.clone()
                    };
                    screen.write_str_wrapped_clipped(
                        start_row,
                        content_indent,
                        &display_text,
                        styles::THINKING_FG,
                        Color::Default,
                        Style::dim(),
                        content_indent,
                        effective_max_row,
                        wrap_col,
                    );
                } else if skip_top > 0 {
                    let display_text = if text.is_empty() {
                        "⋯".to_string()
                    } else {
                        text.clone()
                    };
                    screen.write_str_wrapped_skip_clipped(
                        start_row,
                        content_indent,
                        &display_text,
                        styles::THINKING_FG,
                        Color::Default,
                        Style::dim(),
                        content_indent,
                        effective_max_row,
                        wrap_col,
                        skip_top,
                    );
                }
            }
            ConversationLine::Separator => {
                if skip_top == 0 && start_row <= max_row {
                    screen.hline(
                        start_row,
                        area.x + 2,
                        area.x + area.width.saturating_sub(4),
                        '─',
                        Color::Ansi(240),
                        Color::Default,
                    );
                }
            }
            ConversationLine::ConfirmPrompt {
                name,
                args_summary,
                diff_preview,
            } => {
                let mut current_row = start_row;
                let mut lines_remaining = rows_available;

                // Render the prompt line
                if skip_top == 0 && current_row <= max_row && lines_remaining > 0 {
                    let prompt = format!("  ⚠ Confirm {} {}", name, args_summary);
                    let suffix = " [y/n/a]?";
                    screen.write_str(
                        current_row,
                        area.x,
                        &prompt,
                        Color::YELLOW,
                        Color::Default,
                        Style::bold(),
                    );
                    let prompt_end =
                        area.x + prompt.len().min(area.width as usize - suffix.len()) as u16;
                    screen.write_str(
                        current_row,
                        prompt_end,
                        suffix,
                        Color::YELLOW,
                        Color::Default,
                        Style::default(),
                    );
                    current_row += 1;
                    lines_remaining = lines_remaining.saturating_sub(1);
                }

                // Render diff preview lines
                if let Some(diff) = diff_preview
                    && !diff.is_empty()
                {
                    let max_content_width = (area.width as usize).saturating_sub(5);
                    for line in diff.lines() {
                        if lines_remaining == 0 || current_row > max_row {
                            break;
                        }

                        // Determine color based on diff prefix (enhanced)
                        let (line_color, line_bg, prefix) = if line.starts_with("@@") {
                            // Hunk header
                            (Color::CYAN, Color::Default, "  │ ")
                        } else if line.starts_with("---") || line.starts_with("+++") {
                            // File header
                            (Color::WHITE, Color::Default, "  │ ")
                        } else if line.starts_with('-') {
                            // Removed line — red text with dark red background
                            (Color::RED, Color::Ansi(52), "  │ ")
                        } else if line.starts_with('+') {
                            // Added line — green text with dark green background
                            (Color::GREEN, Color::Ansi(22), "  │ ")
                        } else {
                            (Color::Ansi(252), Color::Default, "  │ ")
                        };

                        let display = if line.is_empty() {
                            prefix.to_string()
                        } else if line.width() > max_content_width {
                            format!(
                                "{}{}…",
                                prefix,
                                truncate_str_width(line, max_content_width.saturating_sub(1))
                            )
                        } else {
                            format!("{}{}", prefix, line)
                        };

                        screen.write_str(
                            current_row,
                            area.x,
                            &display,
                            line_color,
                            line_bg,
                            Style::default(),
                        );
                        // Fill background for diff lines with colored backgrounds
                        if line_bg != Color::Default {
                            let fill_end = area.x + area.width.saturating_sub(1);
                            let end_col =
                                area.x + display.width_cjk().min(area.width as usize - 1) as u16;
                            if end_col < fill_end {
                                for c in end_col..fill_end {
                                    if let Some(cell) = screen.get_mut(current_row, c) {
                                        cell.bg = line_bg;
                                    }
                                }
                            }
                        }

                        current_row += 1;
                        lines_remaining = lines_remaining.saturating_sub(1);
                    }
                }
            }
            ConversationLine::Question { question, answers } => {
                // Question header: "  ❓ <question>"
                let question_prefix = "  ❓ ";
                let answer_prefix_len = if answers.len() >= 10 { 7 } else { 6 };
                let mut current_row = start_row;
                let mut lines_remaining = rows_available;

                // Render question line
                if skip_top == 0 && current_row <= max_row && lines_remaining > 0 {
                    screen.write_str(
                        current_row,
                        area.x,
                        question_prefix,
                        Color::YELLOW,
                        Color::Default,
                        Style::bold(),
                    );
                    screen.write_str_wrapped_clipped(
                        current_row,
                        area.x + question_prefix.len() as u16,
                        question,
                        Color::YELLOW,
                        Color::Default,
                        Style::default(),
                        area.x + question_prefix.len() as u16,
                        effective_max_row,
                        wrap_col,
                    );
                    current_row += 1;
                    lines_remaining -= 1;
                } else if skip_top > 0 {
                    // Skip question lines
                    let q_height = self.line_height_for_text(
                        question,
                        area.width,
                        question_prefix.len(),
                        question_prefix.len(),
                    );
                    if skip_top >= q_height {
                        // Question entirely skipped; adjust for remaining skip
                    }
                }

                // Render answer lines: "    N. <answer>"
                for (i, answer) in answers.iter().enumerate() {
                    let answer_label = format!("    {}. ", i + 1);
                    let a_height = self.line_height_for_text(
                        answer,
                        area.width,
                        answer_prefix_len,
                        answer_prefix_len,
                    );

                    if skip_top > 0 {
                        // Still skipping
                        if a_height <= skip_top {
                            // This answer is entirely skipped
                        } else {
                            // Partially visible — render with skip
                            if current_row <= max_row && lines_remaining > 0 {
                                screen.write_str(
                                    current_row,
                                    area.x,
                                    &answer_label,
                                    Color::CYAN,
                                    Color::Default,
                                    Style::bold(),
                                );
                                screen.write_str_wrapped_skip_clipped(
                                    current_row,
                                    area.x + answer_label.len() as u16,
                                    answer,
                                    Color::WHITE,
                                    Color::Default,
                                    Style::default(),
                                    area.x + answer_label.len() as u16,
                                    effective_max_row,
                                    wrap_col,
                                    skip_top,
                                );
                            }
                        }
                    } else if current_row <= max_row && lines_remaining > 0 {
                        // Normal rendering (no skipping)
                        screen.write_str(
                            current_row,
                            area.x,
                            &answer_label,
                            Color::CYAN,
                            Color::Default,
                            Style::bold(),
                        );
                        screen.write_str_wrapped_clipped(
                            current_row,
                            area.x + answer_label.len() as u16,
                            answer,
                            Color::WHITE,
                            Color::Default,
                            Style::default(),
                            area.x + answer_label.len() as u16,
                            effective_max_row,
                            wrap_col,
                        );
                        current_row += 1;
                        lines_remaining = lines_remaining.saturating_sub(1);
                    }
                }
            }
        }
    }

    /// Render the context warning banner at the top of the conversation area.
    fn render_context_warning(&self, area: Rect, screen: &mut Screen) {
        let row = area.y;
        let (fg, bg, msg) = match &self.context_warning {
            ContextWarningLevel::None => return,
            ContextWarningLevel::Warning(pct) => (
                Color::YELLOW,
                Color::Ansi(17), // dark blue bg
                format!(
                    " ⚠ Context {:.0}% full — consider /compact to free space ",
                    pct
                ),
            ),
            ContextWarningLevel::Critical(pct) => (
                Color::WHITE,
                Color::Ansi(52), // dark red bg
                format!(" ⚠ Context {:.0}% — exceeded! Use /compact now ", pct),
            ),
        };

        // Fill the banner row with background color
        for col in area.x..area.x + area.width {
            if let Some(cell) = screen.get_mut(row, col) {
                cell.bg = bg;
                cell.char = ' ';
            }
        }

        // Write the warning text (truncate if wider than area)
        let display = if msg.width() > area.width as usize {
            truncate_str_width(&msg, area.width as usize).to_string()
        } else {
            msg
        };
        screen.write_str(row, area.x, &display, fg, bg, Style::bold());
    }

    /// Render the search bar at the bottom of the conversation area.
    fn render_search_bar(&self, row: u16, area: Rect, screen: &mut Screen) {
        // Fill the search bar background
        for col in area.x..area.x + area.width {
            if let Some(cell) = screen.get_mut(row, col) {
                cell.bg = styles::STATUS_BAR_BG;
                cell.char = ' ';
            }
        }

        // "Search: " label
        let label = " Search: ";
        screen.write_str(
            row,
            area.x,
            label,
            Color::YELLOW,
            styles::STATUS_BAR_BG,
            Style::bold(),
        );

        let query_col = area.x + label.len() as u16;

        // Search query text
        let query_text = &self.search.query;
        let max_query_width = area.width.saturating_sub(label.len() as u16 + 20); // reserve for match count
        let display_query = if query_text.width() > max_query_width as usize {
            truncate_str_width(query_text, max_query_width as usize).to_string()
        } else {
            query_text.clone()
        };
        screen.write_str(
            row,
            query_col,
            &display_query,
            Color::WHITE,
            styles::STATUS_BAR_BG,
            Style::default(),
        );

        // Cursor indicator (underline the character at cursor position or show block if at end)
        let cursor_col_in_display = if self.search.cursor > query_text.len() {
            display_query.width()
        } else {
            // Find the display position corresponding to the cursor byte offset
            let before_cursor = &query_text[..self.search.cursor.min(query_text.len())];
            let display_offset = before_cursor.width();
            if display_offset > max_query_width as usize {
                max_query_width as usize
            } else {
                display_offset
            }
        };
        let cursor_screen_col = area.x + label.len() as u16 + cursor_col_in_display as u16;
        if cursor_screen_col < area.x + area.width {
            if let Some(cell) = screen.get_mut(row, cursor_screen_col) {
                cell.style.underline = true;
                if cell.char == ' ' {
                    cell.fg = Color::WHITE;
                }
            }
        }

        // Match count on the right side
        let match_count = if !self.search.match_rows.is_empty() {
            format!(
                " {}/{} ",
                self.search.current_match + 1,
                self.search.match_rows.len()
            )
        } else if !self.search.query.is_empty() {
            " No matches ".to_string()
        } else {
            String::new()
        };

        if !match_count.is_empty() {
            let match_start = area
                .x
                .saturating_add(area.width)
                .saturating_sub(match_count.len() as u16);
            screen.write_str(
                row,
                match_start,
                &match_count,
                Color::Ansi(244),
                styles::STATUS_BAR_BG,
                Style::dim(),
            );
        }
    }

    /// Highlight search matches in the visible area of the conversation.
    fn highlight_search_matches(&self, area: Rect, screen: &mut Screen, content_width: u16) {
        if self.search.query.is_empty() {
            return;
        }

        let query_lower = self.search.query.to_lowercase();
        let visible_start = self.scroll_offset;
        let visible_end = visible_start + area.height as usize;

        // Walk through conversation lines, computing visual row offsets,
        // and highlight matches in visible lines.
        let mut visual_row = 0usize;
        let mut screen_row = area.y;
        let mut match_idx = 0usize;

        for line in &self.lines {
            let height = self.line_height(line, content_width);

            // Skip lines entirely above the visible area
            if visual_row + height <= visible_start {
                visual_row += height;
                continue;
            }

            // Stop if we're past the visible area
            if visual_row >= visible_end {
                break;
            }

            let skip_top = visible_start.saturating_sub(visual_row);
            let rows_available = area.bottom().saturating_sub(screen_row) as usize;
            if rows_available == 0 {
                break;
            }

            // Get the searchable text for this line
            let text = match line {
                ConversationLine::User { text } => text.clone(),
                ConversationLine::Assistant { text } => text.clone(),
                ConversationLine::System { text } => text.clone(),
                ConversationLine::Thinking { text } => text.clone(),
                ConversationLine::ToolResult { content, .. } => content.clone(),
                ConversationLine::ToolCall { name, args_summary } => {
                    format!("{} {}", name, args_summary)
                }
                ConversationLine::ConfirmPrompt {
                    name, args_summary, ..
                } => {
                    format!("{} {}", name, args_summary)
                }
                ConversationLine::Question { question, answers } => {
                    format!("{} {}", question, answers.join(" "))
                }
                ConversationLine::Separator => String::new(),
            };

            if !text.is_empty() {
                let text_lower = text.to_lowercase();
                let mut search_start = 0usize;
                while let Some(pos) = text_lower[search_start..].find(&query_lower) {
                    let abs_pos = search_start + pos;
                    search_start = abs_pos + query_lower.len();

                    // Compute the screen column for this match
                    // The match may span multiple visual rows due to wrapping
                    let prefix = &text[..abs_pos];
                    let prefix_chars: Vec<char> = prefix.chars().collect();
                    let query_chars: Vec<char> = self.search.query.chars().collect();
                    let _query_len = query_chars.len();

                    // Determine which visual row within this line the match starts on
                    // Use the same wrapping logic as render (simplified)
                    let line_prefix_len = match line {
                        ConversationLine::User { .. } => 7,
                        ConversationLine::Assistant { .. } => 7,
                        ConversationLine::System { .. } => 2,
                        ConversationLine::Thinking { .. } => 13,
                        _ => 4, // tool result, etc.
                    };
                    let wrap_at = content_width as usize;

                    // Calculate the visual row offset for the start of the match
                    let mut row_offset = 0usize;
                    let mut col = line_prefix_len;
                    for ch in prefix_chars.iter() {
                        let w = ch.width().unwrap_or(1).max(1) as usize;
                        col += w;
                        if col > wrap_at {
                            row_offset += 1;
                            col = line_prefix_len + w;
                        }
                    }

                    // Calculate the starting column for the match
                    let start_col = col;

                    // Check if this match is visible (considering skip_top)
                    let actual_visual_row = visual_row + row_offset;
                    if actual_visual_row >= visible_end {
                        break;
                    }
                    if actual_visual_row < visible_start {
                        // Match is above visible area but might span into it
                        // For simplicity, skip matches that start above the visible area
                        continue;
                    }

                    let match_screen_row = screen_row + row_offset as u16;
                    if skip_top > 0 && row_offset < skip_top {
                        continue;
                    }

                    // Determine if this is the current match
                    let is_current = match_idx < self.search.match_rows.len()
                        && self.search.match_rows[match_idx] == visual_row;

                    // Highlight the match characters on screen
                    let highlight_fg = if is_current {
                        Color::BLACK
                    } else {
                        Color::WHITE
                    };
                    let highlight_bg = if is_current {
                        Color::ORANGE
                    } else {
                        Color::Ansi(58) // dark yellow/olive
                    };

                    let mut highlight_col = area.x + start_col as u16;
                    for ch in query_chars.iter() {
                        if highlight_col >= area.x + area.width.saturating_sub(1) {
                            break; // Don't overwrite scrollbar
                        }
                        if let Some(cell) = screen.get_mut(match_screen_row, highlight_col) {
                            cell.fg = highlight_fg;
                            cell.bg = highlight_bg;
                        }
                        highlight_col += ch.width().unwrap_or(1).max(1) as u16;
                    }

                    match_idx += 1;
                }
            }

            screen_row += height.saturating_sub(skip_top) as u16;
            screen_row = screen_row.min(area.bottom());
            visual_row += height;
        }
    }

    /// Render a scrollbar on the right edge of the area.
    fn render_scrollbar(&self, area: Rect, screen: &mut Screen, total_lines: usize) {
        if total_lines <= area.height as usize {
            return;
        }

        let scrollbar_height = area.height;
        let thumb_size =
            ((scrollbar_height as usize * scrollbar_height as usize) / total_lines).max(1) as u16;
        let thumb_position = if total_lines > area.height as usize {
            (self.scroll_offset as u16 * (scrollbar_height - thumb_size))
                / (total_lines as u16 - area.height)
        } else {
            0
        };

        let x = area.x + area.width - 1;
        for row in area.y..area.y + area.height {
            if let Some(cell) = screen.get_mut(row, x) {
                cell.char = '│';
                cell.fg = styles::SCROLLBAR_FG;
                cell.bg = styles::SCROLLBAR_BG;
            }
        }

        for i in 0..thumb_size {
            let row = area.y + thumb_position + i;
            if row < area.y + area.height {
                if let Some(cell) = screen.get_mut(row, x) {
                    cell.char = '█';
                    cell.fg = styles::SCROLLBAR_FG;
                }
            }
        }
    }
}

impl Widget for ConversationWidget {
    fn render(&mut self, area: Rect, screen: &mut Screen) {
        if area.is_empty() {
            return;
        }

        screen.fill_rect(area, Cell::default());

        // Reserve space for search bar if active (1 row at bottom of conversation)
        let search_active = self.search.active;
        let search_bar_rows: u16 = if search_active { 1 } else { 0 };

        // Reserve space for context warning banner (1 row at top)
        let warning_banner_rows: u16 = if matches!(self.context_warning, ContextWarningLevel::None)
        {
            0
        } else {
            1
        };

        let content_area = Rect::new(
            area.x,
            area.y + warning_banner_rows,
            area.width,
            area.height
                .saturating_sub(warning_banner_rows + search_bar_rows),
        );

        // Render context warning banner
        if warning_banner_rows > 0 {
            self.render_context_warning(area, screen);
        }

        // Render search bar at the bottom of the conversation area
        if search_active {
            let search_row = area.y + area.height.saturating_sub(1);
            self.render_search_bar(search_row, area, screen);
        }

        let visible_rows = content_area.height as usize;
        // Reserve 1 column for the scrollbar
        let content_width = content_area.width.saturating_sub(1);
        let total_height = self.total_visual_height(content_width);

        let max_scroll = total_height.saturating_sub(visible_rows);
        if self.auto_scroll {
            self.scroll_offset = max_scroll;
        }
        if self.scroll_offset > max_scroll {
            self.scroll_offset = max_scroll;
        }

        let mut visual_row = 0usize;
        let mut screen_row = content_area.y;
        let skip_rows = self.scroll_offset;

        for line in &self.lines {
            let height = self.line_height(line, content_width);

            if visual_row + height <= skip_rows {
                visual_row += height;
                continue;
            }

            let skip_top = skip_rows.saturating_sub(visual_row);
            let rows_available = content_area.bottom().saturating_sub(screen_row) as usize;

            if rows_available == 0 {
                break;
            }

            self.render_line_clipped(
                line,
                screen_row,
                screen,
                content_width,
                content_area,
                skip_top,
                rows_available,
            );

            screen_row += height.saturating_sub(skip_top) as u16;
            screen_row = screen_row.min(content_area.bottom());
            visual_row += height;

            if screen_row >= content_area.bottom() {
                break;
            }
        }

        self.render_scrollbar(content_area, screen, total_height);

        // Highlight search matches if active
        if self.search.active && !self.search.query.is_empty() {
            self.highlight_search_matches(content_area, screen, content_width);
        }
    }

    fn handle_event(&mut self, event: &Event) -> Action {
        // If search mode is active, intercept all key events
        if self.search.active {
            if let Event::Key(key) = event {
                match key {
                    KeyEvent {
                        key: Key::Escape,
                        modifiers,
                    } if !modifiers.ctrl && !modifiers.alt => {
                        self.close_search();
                        return Action::None;
                    }
                    KeyEvent {
                        key: Key::Enter,
                        modifiers,
                    } if modifiers.shift => {
                        // Shift+Enter: previous match
                        self.search_prev();
                        return Action::None;
                    }
                    KeyEvent {
                        key: Key::Enter,
                        modifiers,
                    } if !modifiers.ctrl && !modifiers.alt => {
                        // Enter: next match
                        self.search_next();
                        return Action::None;
                    }
                    KeyEvent {
                        key: Key::Char(ch),
                        modifiers,
                    } if !modifiers.ctrl && !modifiers.alt => {
                        self.search_type_char(*ch);
                        return Action::None;
                    }
                    KeyEvent {
                        key: Key::Backspace,
                        ..
                    } => {
                        self.search_backspace();
                        return Action::None;
                    }
                    KeyEvent {
                        key: Key::Left,
                        modifiers,
                    } if !modifiers.ctrl && !modifiers.alt => {
                        self.search_cursor_left();
                        return Action::None;
                    }
                    KeyEvent {
                        key: Key::Right,
                        modifiers,
                    } if !modifiers.ctrl && !modifiers.alt => {
                        self.search_cursor_right();
                        return Action::None;
                    }
                    _ => {
                        // Let other keys pass through (e.g., Ctrl+C for quit)
                        let _ = key;
                    }
                }
            }
            // Non-key events pass through
            return Action::None;
        }

        if let Event::Key(key) = event {
            match key {
                KeyEvent {
                    key: Key::Char('f'),
                    modifiers,
                } if modifiers.ctrl => {
                    self.toggle_search();
                    return Action::None;
                }
                KeyEvent {
                    key: Key::Tab,
                    modifiers,
                } if !modifiers.shift && !modifiers.alt && !modifiers.ctrl => {
                    return Action::CycleFocusForward;
                }
                KeyEvent {
                    key: Key::BackTab, ..
                } => {
                    return Action::CycleFocusBackward;
                }
                _ => {}
            }
        }
        let _ = event;
        Action::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conversation_new() {
        let conv = ConversationWidget::new();
        assert!(conv.lines.is_empty());
        assert!(conv.auto_scroll);
    }

    #[test]
    fn test_conversation_push() {
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::User {
            text: "Hello".to_string(),
        });
        conv.push(ConversationLine::Assistant {
            text: "Hi there!".to_string(),
        });
        assert_eq!(conv.lines.len(), 2);
    }

    #[test]
    fn test_conversation_scroll() {
        let mut conv = ConversationWidget::new();
        for i in 0..50 {
            conv.push(ConversationLine::User {
                text: format!("Message {}", i),
            });
        }
        assert!(conv.auto_scroll);

        conv.scroll_up(10);
        assert!(!conv.auto_scroll);

        conv.scroll_down(10);
    }

    #[test]
    fn test_conversation_clear() {
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::User {
            text: "Hello".to_string(),
        });
        conv.clear();
        assert!(conv.lines.is_empty());
    }

    #[test]
    fn test_conversation_render() {
        let mut screen = Screen::new(80, 24);
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::User {
            text: "Hello".to_string(),
        });
        conv.push(ConversationLine::Separator);
        conv.push(ConversationLine::Assistant {
            text: "Hi there!".to_string(),
        });

        let area = Rect::new(0, 0, 80, 20);
        conv.render(area, &mut screen);

        assert!(screen.get(0, 0).unwrap().char != '\0');
    }

    #[test]
    fn test_tool_result_line_height() {
        let conv = ConversationWidget::new();
        let line = ConversationLine::ToolResult {
            name: "read".to_string(),
            content: "line1\nline2\nline3".to_string(),
            is_error: false,
        };
        assert_eq!(conv.line_height(&line, 80), 3);
    }

    #[test]
    fn test_tool_result_trailing_newlines() {
        let conv = ConversationWidget::new();
        // Trailing newlines should be trimmed
        let line = ConversationLine::ToolResult {
            name: "read".to_string(),
            content: "line1\nline2\n\n\n".to_string(),
            is_error: false,
        };
        // Should count 2 visible lines, not 4
        assert_eq!(conv.line_height(&line, 80), 2);
    }

    #[test]
    fn test_tool_result_truncated() {
        let conv = ConversationWidget::new();
        // Content with more than 20 lines should be capped
        let long_content: String = (0..30)
            .map(|i| format!("line {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let line = ConversationLine::ToolResult {
            name: "run".to_string(),
            content: long_content,
            is_error: false,
        };
        // Should be capped at 20 lines + truncation indicator
        let height = conv.line_height(&line, 80);
        assert!(
            height <= 21,
            "Tool result should be capped at ~20 lines, got {}",
            height
        );
    }

    #[test]
    fn test_tool_result_single_line() {
        let conv = ConversationWidget::new();
        let line = ConversationLine::ToolResult {
            name: "read".to_string(),
            content: "single line".to_string(),
            is_error: false,
        };
        assert_eq!(conv.line_height(&line, 80), 1);
    }

    #[test]
    fn test_assistant_wrapping_line_height() {
        let conv = ConversationWidget::new();
        // With width=20, prefix "  AI:  " (7 chars), text "12345678901234567890" (20 chars)
        // First row: prefix + 13 chars, second row: 7 more chars
        let line = ConversationLine::Assistant {
            text: "12345678901234567890".to_string(),
        };
        let height = conv.line_height(&line, 20);
        assert!(
            height >= 2,
            "Long text should wrap to multiple rows, got {}",
            height
        );
    }

    #[test]
    fn test_confirm_prompt_line_height() {
        let conv = ConversationWidget::new();
        let line = ConversationLine::ConfirmPrompt {
            name: "run".to_string(),
            args_summary: "cargo test".to_string(),
            diff_preview: None,
        };
        // ConfirmPrompt with no diff takes 1 row
        assert_eq!(conv.line_height(&line, 80), 1);
    }

    #[test]
    fn test_confirm_prompt_with_diff_line_height() {
        let conv = ConversationWidget::new();
        let diff = "-  old line\n+  new line\n  context line".to_string();
        let line = ConversationLine::ConfirmPrompt {
            name: "edit".to_string(),
            args_summary: "src/main.rs".to_string(),
            diff_preview: Some(diff),
        };
        // 1 row for prompt + 3 rows for diff lines
        assert_eq!(conv.line_height(&line, 80), 4);
    }

    #[test]
    fn test_confirm_prompt_push() {
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::ConfirmPrompt {
            name: "run".to_string(),
            args_summary: "cargo build".to_string(),
            diff_preview: None,
        });
        assert_eq!(conv.lines.len(), 1);
    }

    // ── Search tests ────────────────────────────────────────────────────

    #[test]
    fn test_search_toggle() {
        let mut conv = ConversationWidget::new();
        assert!(!conv.is_search_active());
        conv.toggle_search();
        assert!(conv.is_search_active());
        conv.toggle_search();
        assert!(!conv.is_search_active());
    }

    #[test]
    fn test_search_close() {
        let mut conv = ConversationWidget::new();
        conv.toggle_search();
        assert!(conv.is_search_active());
        conv.close_search();
        assert!(!conv.is_search_active());
    }

    #[test]
    fn test_search_type_char_and_backspace() {
        let mut conv = ConversationWidget::new();
        conv.toggle_search();
        conv.search_type_char('h');
        conv.search_type_char('i');
        assert_eq!(conv.search.query, "hi");
        assert_eq!(conv.search.cursor, 2);
        conv.search_backspace();
        assert_eq!(conv.search.query, "h");
        assert_eq!(conv.search.cursor, 1);
        conv.search_backspace();
        assert_eq!(conv.search.query, "");
        assert_eq!(conv.search.cursor, 0);
        // Backspace on empty query is a no-op
        conv.search_backspace();
        assert_eq!(conv.search.query, "");
    }

    #[test]
    fn test_search_cursor_movement() {
        let mut conv = ConversationWidget::new();
        conv.toggle_search();
        conv.search_type_char('a');
        conv.search_type_char('b');
        conv.search_type_char('c');
        assert_eq!(conv.search.cursor, 3);
        conv.search_cursor_left();
        assert_eq!(conv.search.cursor, 2);
        conv.search_cursor_left();
        assert_eq!(conv.search.cursor, 1);
        conv.search_cursor_right();
        assert_eq!(conv.search.cursor, 2);
        // Can't go past the end
        conv.search_cursor_right();
        conv.search_cursor_right();
        assert_eq!(conv.search.cursor, 3);
    }

    #[test]
    fn test_search_matches() {
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::User {
            text: "Hello world".to_string(),
        });
        conv.push(ConversationLine::Assistant {
            text: "Hi there!".to_string(),
        });
        conv.push(ConversationLine::User {
            text: "Hello again".to_string(),
        });
        conv.toggle_search();
        conv.search_type_char('h');
        conv.search_type_char('e');
        conv.search_type_char('l');
        // "hel" matches "Hello world" and "Hello again" (case-insensitive)
        assert_eq!(conv.search.match_rows.len(), 2);
    }

    #[test]
    fn test_search_no_matches() {
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::User {
            text: "Hello world".to_string(),
        });
        conv.toggle_search();
        conv.search_type_char('z');
        conv.search_type_char('z');
        conv.search_type_char('z');
        assert!(conv.search.match_rows.is_empty());
    }

    #[test]
    fn test_search_navigation() {
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::User {
            text: "First match".to_string(),
        });
        conv.push(ConversationLine::Assistant {
            text: "No hit here".to_string(),
        });
        conv.push(ConversationLine::User {
            text: "Second match".to_string(),
        });
        conv.toggle_search();
        conv.search_type_char('m');
        conv.search_type_char('a');
        conv.search_type_char('t');
        conv.search_type_char('c');
        conv.search_type_char('h');
        assert_eq!(conv.search.match_rows.len(), 2);
        assert_eq!(conv.search.current_match, 0);
        conv.search_next();
        assert_eq!(conv.search.current_match, 1);
        conv.search_next();
        assert_eq!(conv.search.current_match, 0); // wraps around
        conv.search_prev();
        assert_eq!(conv.search.current_match, 1); // wraps around
    }

    #[test]
    fn test_search_clears_on_close() {
        let mut conv = ConversationWidget::new();
        conv.toggle_search();
        conv.search_type_char('x');
        conv.close_search();
        assert!(!conv.is_search_active());
        assert!(conv.search.query.is_empty());
        assert!(conv.search.match_rows.is_empty());
    }

    // ── Context warning tests ──────────────────────────────────────────

    #[test]
    fn test_context_warning_default() {
        let conv = ConversationWidget::new();
        assert!(matches!(conv.context_warning(), ContextWarningLevel::None));
    }

    #[test]
    fn test_context_warning_set_warning() {
        let mut conv = ConversationWidget::new();
        conv.set_context_warning(ContextWarningLevel::Warning(75.0));
        assert!(matches!(
            conv.context_warning(),
            ContextWarningLevel::Warning(75.0)
        ));
    }

    #[test]
    fn test_context_warning_set_critical() {
        let mut conv = ConversationWidget::new();
        conv.set_context_warning(ContextWarningLevel::Critical(92.0));
        assert!(matches!(
            conv.context_warning(),
            ContextWarningLevel::Critical(92.0)
        ));
    }

    #[test]
    fn test_context_warning_render() {
        let mut screen = Screen::new(80, 24);
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::User {
            text: "Hello".to_string(),
        });
        conv.set_context_warning(ContextWarningLevel::Warning(75.0));
        let area = Rect::new(0, 0, 80, 20);
        conv.render(area, &mut screen);
        // First row should have the warning banner (non-default background)
        let first_cell = screen.get(0, 0).unwrap();
        assert_ne!(first_cell.bg, Color::Default);
        assert_eq!(first_cell.char, ' ');
    }

    #[test]
    fn test_context_warning_critical_render() {
        let mut screen = Screen::new(80, 24);
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::User {
            text: "Hello".to_string(),
        });
        conv.set_context_warning(ContextWarningLevel::Critical(95.0));
        let area = Rect::new(0, 0, 80, 20);
        conv.render(area, &mut screen);
        let first_cell = screen.get(0, 0).unwrap();
        assert_eq!(first_cell.bg, Color::Ansi(52)); // dark red bg for critical
    }

    // ── Diff coloring tests ─────────────────────────────────────────────

    #[test]
    fn test_diff_coloring_in_tool_result() {
        let mut screen = Screen::new(80, 24);
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::ToolResult {
            name: "edit".to_string(),
            content:
                "--- a/file.rs\n+++ b/file.rs\n@@ -1,3 +1,3 @@\n- old line\n+ new line\n  context"
                    .to_string(),
            is_error: false,
        });
        let area = Rect::new(0, 0, 80, 20);
        conv.render(area, &mut screen);
        // Just verify it doesn't panic and renders something
        assert!(screen.get(0, 0).unwrap().char != '\0');
    }

    #[test]
    fn test_diff_coloring_in_confirm_prompt() {
        let mut screen = Screen::new(80, 24);
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::ConfirmPrompt {
            name: "edit".to_string(),
            args_summary: "src/main.rs".to_string(),
            diff_preview: Some("@@ -1,3 +1,3 @@\n- old line\n+ new line".to_string()),
        });
        let area = Rect::new(0, 0, 80, 20);
        conv.render(area, &mut screen);
        assert!(screen.get(0, 0).unwrap().char != '\0');
    }

    #[test]
    fn test_search_bar_render() {
        let mut screen = Screen::new(80, 24);
        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::User {
            text: "Hello".to_string(),
        });
        conv.toggle_search();
        conv.search_type_char('H');
        let area = Rect::new(0, 0, 80, 20);
        conv.render(area, &mut screen);
        // Search bar should be at the bottom row (row 19)
        let search_bar_row = 19u16;
        let cell = screen.get(search_bar_row, 1).unwrap();
        assert_eq!(cell.char, 'S'); // "Search: " starts with S
    }
}
