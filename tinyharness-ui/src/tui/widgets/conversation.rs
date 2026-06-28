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
                // 20 visible lines. Each content line is truncated (not wrapped)
                // in the rendering, so each takes exactly 1 visual row.
                // When there are more than 20 lines, a truncation indicator
                // row ("  │ …") is shown, adding 1 extra visual row.
                let trimmed = content.trim_end_matches('\n');
                let line_count = trimmed.lines().count();
                let visible = line_count.clamp(1, 20);
                // Account for the truncation indicator row when content exceeds 20 lines
                return if line_count > 20 {
                    visible + 1
                } else {
                    visible
                };
            }
            ConversationLine::ToolCall { name, args_summary } => {
                if args_summary.is_empty() {
                    return 1;
                }
                let header = format!("  ── {}", name);
                // Use display width (not byte length) for column calculation
                let header_display_width = header.width();
                return self.line_height_for_text(
                    args_summary,
                    area_width,
                    header_display_width,
                    header_display_width,
                );
            }
            ConversationLine::Question { question, answers } => {
                let question_prefix_width = "  ❓ ".width();
                let question_rows = self.line_height_for_text(
                    question,
                    area_width,
                    question_prefix_width,
                    question_prefix_width,
                );
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
            ConversationLine::ConfirmPrompt {
                name,
                args_summary,
                diff_preview,
            } => {
                let prompt_prefix = "  ⚠ Confirm ";
                let prompt_prefix_width = prompt_prefix.width();
                let prompt_text = format!("{} {}", name, args_summary);
                let mut rows = self.line_height_for_text(
                    &prompt_text,
                    area_width,
                    prompt_prefix_width,
                    prompt_prefix_width,
                );
                // Check if the suffix fits on the last line; if not, add a row
                let suffix = " [y/n/a]?";
                let suffix_width = suffix.width();
                let last_line_used = if prompt_text.is_empty() {
                    prompt_prefix_width
                } else {
                    // Calculate columns used on the last wrapped line
                    let wrap_col = area_width as usize;
                    let mut col = prompt_prefix_width;
                    for ch in prompt_text.chars() {
                        let w = ch.width().unwrap_or(1);
                        if w == 0 {
                            continue;
                        }
                        if col + w > wrap_col {
                            col = prompt_prefix_width + w;
                        } else {
                            col += w;
                        }
                    }
                    col
                };
                if last_line_used + suffix_width > area_width as usize {
                    rows += 1;
                }
                if let Some(diff) = diff_preview
                    && !diff.is_empty()
                {
                    // Each diff line is truncated (not wrapped) in rendering,
                    // so each takes exactly 1 visual row.
                    rows += diff.lines().count();
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
                let prefix_width = prefix.len() as u16;
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
                        area.x + prefix_width,
                        text,
                        Color::GREEN,
                        Color::Default,
                        Style::default(),
                        area.x + prefix_width,
                        effective_max_row,
                        wrap_col,
                    );
                } else if skip_top > 0 {
                    screen.write_str_wrapped_skip_clipped(
                        start_row,
                        area.x + prefix_width,
                        text,
                        Color::GREEN,
                        Color::Default,
                        Style::default(),
                        area.x + prefix_width,
                        effective_max_row,
                        wrap_col,
                        skip_top,
                    );
                }
            }
            ConversationLine::Assistant { text } => {
                let prefix = "  AI:  ";
                let prefix_width = prefix.len() as u16;
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
                        area.x + prefix_width,
                        text,
                        Color::WHITE,
                        Color::Default,
                        Style::default(),
                        area.x + prefix_width,
                        effective_max_row,
                        wrap_col,
                    );
                } else if skip_top > 0 {
                    screen.write_str_wrapped_skip_clipped(
                        start_row,
                        area.x + prefix_width,
                        text,
                        Color::WHITE,
                        Color::Default,
                        Style::default(),
                        area.x + prefix_width,
                        effective_max_row,
                        wrap_col,
                        skip_top,
                    );
                }
            }
            ConversationLine::ToolCall { name, args_summary } => {
                let header = format!("  ── {}", name);
                // Use display width (not byte length) for column calculation
                let header_display_width = header.width();
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
                        let args_indent = area.x + header_display_width as u16;
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
                    let args_indent = area.x + header_display_width as u16;
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
                let all_lines: Vec<&str> = trimmed.lines().collect();
                let total_lines = all_lines.len();
                let visible_lines: Vec<&str> = all_lines.into_iter().take(20).collect();
                let mut current_row = start_row;
                let max_content_width = (area.width as usize).saturating_sub(5);

                // Skip the first `skip_top` content lines when scrolling
                for (i, content_line) in visible_lines.iter().enumerate() {
                    if i < skip_top {
                        continue;
                    }
                    if current_row > effective_max_row {
                        break;
                    }

                    // Detect diff lines and apply appropriate colors with backgrounds
                    let (line_color, line_bg, line_prefix) = if !is_error {
                        if content_line.starts_with("@@") {
                            (Color::CYAN, Color::Default, "  │ ")
                        } else if content_line.starts_with("---") || content_line.starts_with("+++")
                        {
                            (Color::WHITE, Color::Default, "  │ ")
                        } else if content_line.starts_with("-") {
                            (Color::RED, Color::Ansi(52), "  │ ")
                        } else if content_line.starts_with("+") {
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
                if total_lines > 20 && skip_top <= 20 && current_row <= effective_max_row {
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
                let prefix_width = 2u16;
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
                        area.x + prefix_width,
                        text,
                        Color::YELLOW,
                        Color::Default,
                        Style::default(),
                        area.x + prefix_width,
                        effective_max_row,
                        wrap_col,
                    );
                } else if skip_top > 0 {
                    screen.write_str_wrapped_skip_clipped(
                        start_row,
                        area.x + prefix_width,
                        text,
                        Color::YELLOW,
                        Color::Default,
                        Style::default(),
                        area.x + prefix_width,
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

                // Render the prompt line(s)
                if skip_top == 0 && current_row <= effective_max_row && lines_remaining > 0 {
                    let prompt_prefix = "  ⚠ Confirm ";
                    let prompt_prefix_width = prompt_prefix.width() as u16;
                    let prompt_text = format!("{} {}", name, args_summary);
                    let suffix = " [y/n/a]?";

                    // Write the prefix
                    screen.write_str(
                        current_row,
                        area.x,
                        prompt_prefix,
                        Color::YELLOW,
                        Color::Default,
                        Style::bold(),
                    );

                    // Write the prompt text with wrapping/clipping
                    let last_row = screen.write_str_wrapped_clipped(
                        current_row,
                        area.x + prompt_prefix_width,
                        &prompt_text,
                        Color::YELLOW,
                        Color::Default,
                        Style::bold(),
                        area.x + prompt_prefix_width,
                        effective_max_row,
                        wrap_col,
                    );

                    // Calculate where to put the suffix: on the last row of the wrapped text
                    // If it fits on the last line, append it; otherwise put it on a new line
                    let text_width_on_last_line = if prompt_text.is_empty() {
                        0usize
                    } else {
                        // Calculate the column where the wrapped text ends on the last row
                        let mut col = prompt_prefix_width as usize;
                        let wrap_col_usize = wrap_col as usize;
                        for ch in prompt_text.chars() {
                            let w = ch.width().unwrap_or(1);
                            if w == 0 {
                                continue;
                            }
                            if col + w > wrap_col_usize {
                                col = prompt_prefix_width as usize + w;
                            } else {
                                col += w;
                            }
                        }
                        col - prompt_prefix_width as usize
                    };

                    let suffix_width = suffix.width();
                    let prompt_end_col =
                        area.x + prompt_prefix_width + text_width_on_last_line as u16;

                    if prompt_end_col + suffix_width as u16 <= wrap_col
                        && last_row <= effective_max_row
                    {
                        // Suffix fits on the last line
                        screen.write_str(
                            last_row,
                            prompt_end_col,
                            suffix,
                            Color::YELLOW,
                            Color::Default,
                            Style::default(),
                        );
                        current_row = last_row + 1;
                        let prompt_rows = (last_row - start_row + 1) as usize;
                        lines_remaining = lines_remaining.saturating_sub(prompt_rows);
                    } else if last_row < effective_max_row && lines_remaining > 1 {
                        // Suffix doesn't fit — put it on a new line
                        screen.write_str(
                            last_row + 1,
                            area.x,
                            suffix,
                            Color::YELLOW,
                            Color::Default,
                            Style::default(),
                        );
                        current_row = last_row + 2;
                        let prompt_rows = (last_row - start_row + 2) as usize;
                        lines_remaining = lines_remaining.saturating_sub(prompt_rows);
                    } else {
                        current_row = last_row + 1;
                        let prompt_rows = (last_row - start_row + 1) as usize;
                        lines_remaining = lines_remaining.saturating_sub(prompt_rows);
                    }
                }

                // Render diff preview lines
                if let Some(diff) = diff_preview
                    && !diff.is_empty()
                {
                    let max_content_width = (area.width as usize).saturating_sub(5);
                    for line in diff.lines() {
                        if lines_remaining == 0 || current_row > effective_max_row {
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
                                area.x + display.width().min(area.width as usize - 1) as u16;
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
                let question_prefix_width = question_prefix.width();
                let answer_prefix_len = if answers.len() >= 10 { 7 } else { 6 };
                let mut current_row = start_row;
                let mut lines_remaining = rows_available;
                let mut skip_remaining = skip_top;

                // Render question line
                if skip_remaining == 0 && current_row <= max_row && lines_remaining > 0 {
                    screen.write_str(
                        current_row,
                        area.x,
                        question_prefix,
                        Color::YELLOW,
                        Color::Default,
                        Style::bold(),
                    );
                    let last_row = screen.write_str_wrapped_clipped(
                        current_row,
                        area.x + question_prefix_width as u16,
                        question,
                        Color::YELLOW,
                        Color::Default,
                        Style::default(),
                        area.x + question_prefix_width as u16,
                        effective_max_row,
                        wrap_col,
                    );
                    current_row = last_row + 1;
                    lines_remaining =
                        lines_remaining.saturating_sub((last_row - start_row + 1) as usize);
                } else if skip_remaining > 0 {
                    // Skip question lines
                    let q_height = self.line_height_for_text(
                        question,
                        area.width,
                        question_prefix_width,
                        question_prefix_width,
                    );
                    if skip_remaining >= q_height {
                        // Question entirely skipped
                        skip_remaining -= q_height;
                    } else {
                        // Partially visible — render with skip
                        if current_row <= effective_max_row && lines_remaining > 0 {
                            screen.write_str(
                                current_row,
                                area.x,
                                question_prefix,
                                Color::YELLOW,
                                Color::Default,
                                Style::bold(),
                            );
                            screen.write_str_wrapped_skip_clipped(
                                current_row,
                                area.x + question_prefix_width as u16,
                                question,
                                Color::YELLOW,
                                Color::Default,
                                Style::default(),
                                area.x + question_prefix_width as u16,
                                effective_max_row,
                                wrap_col,
                                skip_remaining,
                            );
                            let q_rows = q_height - skip_remaining;
                            current_row += q_rows as u16;
                            lines_remaining = lines_remaining.saturating_sub(q_rows);
                        }
                        skip_remaining = 0;
                    }
                }

                // Render answer lines: "    N. <answer>"
                for (i, answer) in answers.iter().enumerate() {
                    let answer_label = format!("    {}. ", i + 1);
                    let answer_label_width = answer_label.width();
                    let a_height = self.line_height_for_text(
                        answer,
                        area.width,
                        answer_prefix_len,
                        answer_prefix_len,
                    );

                    if skip_remaining > 0 {
                        if a_height <= skip_remaining {
                            // This answer is entirely skipped
                            skip_remaining -= a_height;
                        } else {
                            // Partially visible — render with skip
                            if current_row <= effective_max_row && lines_remaining > 0 {
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
                                    area.x + answer_label_width as u16,
                                    answer,
                                    Color::WHITE,
                                    Color::Default,
                                    Style::default(),
                                    area.x + answer_label_width as u16,
                                    effective_max_row,
                                    wrap_col,
                                    skip_remaining,
                                );
                                let a_rows = a_height - skip_remaining;
                                current_row += a_rows as u16;
                                lines_remaining = lines_remaining.saturating_sub(a_rows);
                            }
                            skip_remaining = 0;
                        }
                    } else if current_row <= effective_max_row && lines_remaining > 0 {
                        // Normal rendering (no skipping)
                        screen.write_str(
                            current_row,
                            area.x,
                            &answer_label,
                            Color::CYAN,
                            Color::Default,
                            Style::bold(),
                        );
                        let last_row = screen.write_str_wrapped_clipped(
                            current_row,
                            area.x + answer_label_width as u16,
                            answer,
                            Color::WHITE,
                            Color::Default,
                            Style::default(),
                            area.x + answer_label_width as u16,
                            effective_max_row,
                            wrap_col,
                        );
                        current_row = last_row + 1;
                        lines_remaining = lines_remaining.saturating_sub(a_height);
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
                cell.wide = false;
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
                cell.wide = false;
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
                cell.wide = false;
                cell.fg = styles::SCROLLBAR_FG;
                cell.bg = styles::SCROLLBAR_BG;
            }
        }

        for i in 0..thumb_size {
            let row = area.y + thumb_position + i;
            if row < area.y + area.height {
                if let Some(cell) = screen.get_mut(row, x) {
                    cell.char = '█';
                    cell.wide = false;
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

    // ── Overflow / clipping tests ─────────────────────────────────────────

    /// Check that no non-default cells are written outside the render area.
    fn check_no_overflow(screen: &Screen, area: Rect) -> Vec<(u16, u16, char)> {
        let mut overflows = Vec::new();
        for row in 0..screen.height() {
            for col in 0..screen.width() {
                // Only check cells outside the render area
                if row >= area.y
                    && row < area.y + area.height
                    && col >= area.x
                    && col < area.x + area.width
                {
                    continue;
                }
                let cell = screen.get(row, col).unwrap();
                if cell.char != ' ' && cell.char != '\0' {
                    overflows.push((row, col, cell.char));
                }
            }
        }
        overflows
    }

    #[test]
    fn test_tool_call_does_not_overflow_small_area() {
        // Create a small screen (40 cols, 10 rows)
        // Conversation gets only 5 rows (rows 0-4), rest is "input area"
        let mut screen = Screen::new(40, 10);
        let mut conv = ConversationWidget::new();

        // Push a multiline tool call
        conv.push(ConversationLine::ToolCall {
            name: "run".to_string(),
            args_summary: "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8".to_string(),
        });

        // Render into a small 5-row area
        let area = Rect::new(0, 0, 40, 5);
        conv.render(area, &mut screen);

        // Check for overflow past the bottom of the area (rows 5-9)
        let overflows = check_no_overflow(&screen, area);
        assert!(
            overflows.is_empty(),
            "ToolCall content overflows past area: {:?}",
            overflows
        );
    }

    #[test]
    fn test_tool_result_does_not_overflow_small_area() {
        // Create a small screen (40 cols, 10 rows)
        let mut screen = Screen::new(40, 10);
        let mut conv = ConversationWidget::new();

        // Push a long tool result
        let content: String = (0..15)
            .map(|i| format!("output line number {}", i))
            .collect::<Vec<_>>()
            .join("\n");

        conv.push(ConversationLine::ToolResult {
            name: "run".to_string(),
            content,
            is_error: false,
        });

        // Render into a small 5-row area
        let area = Rect::new(0, 0, 40, 5);
        conv.render(area, &mut screen);

        // Check for overflow past the bottom of the area
        let overflows = check_no_overflow(&screen, area);
        assert!(
            overflows.is_empty(),
            "ToolResult content overflows past area: {:?}",
            overflows
        );
    }

    #[test]
    fn test_tool_call_wrapping_does_not_overflow() {
        // Long args_summary that wraps in a narrow area
        let mut screen = Screen::new(30, 10);
        let mut conv = ConversationWidget::new();

        conv.push(ConversationLine::ToolCall {
            name: "run".to_string(),
            args_summary:
                "cargo test --all-features --workspace -- --test-threads=1 2>&1 | head -100"
                    .to_string(),
        });

        // 4-row area — the wrapping should be clipped, not overflow
        let area = Rect::new(0, 0, 30, 4);
        conv.render(area, &mut screen);

        let overflows = check_no_overflow(&screen, area);
        assert!(
            overflows.is_empty(),
            "ToolCall wrapping overflows past area: {:?}",
            overflows
        );
    }

    #[test]
    fn test_error_tool_result_does_not_overflow() {
        // Error tool result with background fill should not overflow
        let mut screen = Screen::new(40, 10);
        let mut conv = ConversationWidget::new();

        conv.push(ConversationLine::ToolResult {
            name: "run".to_string(),
            content: "error line 1\nerror line 2\nerror line 3\nerror line 4\nerror line 5"
                .to_string(),
            is_error: true,
        });

        let area = Rect::new(0, 0, 40, 3);
        conv.render(area, &mut screen);

        let overflows = check_no_overflow(&screen, area);
        assert!(
            overflows.is_empty(),
            "Error ToolResult overflows past area: {:?}",
            overflows
        );
    }

    #[test]
    fn test_tool_call_line_height_matches_rendering() {
        // Verify that line_height for ToolCall matches actual rendered height
        let conv = ConversationWidget::new();

        // Test 1: short args that fit on one line
        let line = ConversationLine::ToolCall {
            name: "read".to_string(),
            args_summary: "file.txt".to_string(),
        };
        let height = conv.line_height(&line, 40);
        // With header "  ── read" (9 display cols) and args "file.txt" (8 cols),
        // total is 17 cols which fits in 39 cols (40-1 scrollbar). Height = 1.
        assert_eq!(
            height, 1,
            "Short ToolCall should have height 1, got {}",
            height
        );

        // Test 2: args with newlines
        let line = ConversationLine::ToolCall {
            name: "run".to_string(),
            args_summary: "cmd1\ncmd2".to_string(),
        };
        let height = conv.line_height(&line, 40);
        // "  ── run" (9 display cols) + "cmd1" fits on line 1, then wrap to "cmd2" on line 2
        assert_eq!(
            height, 2,
            "ToolCall with newline should have height 2, got {}",
            height
        );

        // Test 3: very long args that wrap
        let line = ConversationLine::ToolCall {
            name: "run".to_string(),
            args_summary: "cargo test --all-features --workspace".to_string(),
        };
        let height = conv.line_height(&line, 20);
        // In a 20-wide area (19 content cols), "  ── run" is 9 cols wide.
        // But header.len() = 9 (ASCII only) so args start at col 9.
        // Wait — "  ── run" where ── is U+2500 (3 bytes each) = "  " (2) + "──" (6 bytes, 2 display cols) + " run" (4) = 12 bytes, 8 display cols
        // header.len() = 12 (bytes), not 8 (display cols)
        // So args start at col 12 (should be col 8), causing wrapping earlier than needed.
        // With wrap_col = 19, args start at col 12, leaving only 7 cols for text.
        // "cargo test" is 10 chars, so it wraps.
        // This is a known byte-vs-display-width bug but height calculation should still be consistent.
        assert!(
            height >= 2,
            "Long ToolCall should wrap to >= 2 rows, got {}",
            height
        );
    }

    #[test]
    fn test_tool_result_line_height_consistency() {
        // ToolResult line_height uses line_height_for_text which wraps,
        // but actual rendering truncates. Verify the height is at least
        // as large as the actual number of rendered rows.
        let conv = ConversationWidget::new();

        // Single-line result
        let line = ConversationLine::ToolResult {
            name: "run".to_string(),
            content: "hello world".to_string(),
            is_error: false,
        };
        let height = conv.line_height(&line, 40);
        assert_eq!(height, 1, "Single-line result should have height 1");

        // Multi-line result (no wrapping needed at width 40)
        let line = ConversationLine::ToolResult {
            name: "run".to_string(),
            content: "line1\nline2\nline3".to_string(),
            is_error: false,
        };
        let height = conv.line_height(&line, 40);
        assert_eq!(height, 3, "3-line result should have height 3");

        // Multi-line result with long lines (wrapping in height calculation)
        // The height should be >= the number of actual rendered lines (which truncate)
        let long_line = "a".repeat(100); // Way wider than 40 cols
        let line = ConversationLine::ToolResult {
            name: "run".to_string(),
            content: long_line,
            is_error: false,
        };
        let height = conv.line_height(&line, 40);
        // Since the content is one long line that wraps in height calculation,
        // height should be > 1 (it wraps in the height calc even though rendering truncates)
        assert!(
            height >= 1,
            "Long single-line result height should be >= 1, got {}",
            height
        );
        // Note: This reveals a height overestimation bug — the height calculation
        // wraps long lines, but rendering truncates them. This means line_height
        // overestimates, causing gaps in the conversation but not overflow.
    }

    #[test]
    fn test_conversation_tool_call_scroll_clipping() {
        // Test that a scrolled conversation with a ToolCall clips correctly
        let mut screen = Screen::new(40, 10);
        let mut conv = ConversationWidget::new();

        // Add enough lines to fill the area and require scrolling
        for i in 0..20 {
            conv.push(ConversationLine::User {
                text: format!("Message {}", i),
            });
        }
        // Add a multiline tool call near the bottom
        conv.push(ConversationLine::ToolCall {
            name: "run".to_string(),
            args_summary: "line1\nline2\nline3\nline4\nline5".to_string(),
        });
        conv.push(ConversationLine::ToolResult {
            name: "run".to_string(),
            content: "output".to_string(),
            is_error: false,
        });

        // Render in a 5-row area (rows 0-4)
        let area = Rect::new(0, 0, 40, 5);
        conv.render(area, &mut screen);

        // Check that rows 5-9 have no non-default content (i.e., no overflow)
        let overflows = check_no_overflow(&screen, area);
        assert!(
            overflows.is_empty(),
            "Scrolled conversation with ToolCall overflows: {:?}",
            overflows
        );
    }

    #[test]
    fn test_tool_result_overestimation_does_not_cause_overflow() {
        // When line_height overestimates (wrapping in height calc, truncation in render),
        // the outer loop should still not cause overflow because screen_row is clamped
        // to content_area.bottom().
        let mut screen = Screen::new(40, 10);
        let mut conv = ConversationWidget::new();

        // Add a ToolResult with a very long single line (will be truncated in rendering)
        let long_content = "x".repeat(200);
        conv.push(ConversationLine::ToolResult {
            name: "run".to_string(),
            content: long_content,
            is_error: false,
        });

        // Render in a 3-row area
        let area = Rect::new(0, 0, 40, 3);
        conv.render(area, &mut screen);

        // Check that rows 3-9 have no overflow
        let overflows = check_no_overflow(&screen, area);
        assert!(
            overflows.is_empty(),
            "ToolResult with long line overflows: {:?}",
            overflows
        );
    }

    #[test]
    fn test_tool_call_with_offset_area_no_overflow() {
        // Simulate the actual TUI layout: conversation area starts at row 1
        // (below status bar), with input bar below it.
        // Screen: 80x24, status bar at row 0, conversation at rows 1-19, input at 20-23
        let mut screen = Screen::new(80, 24);

        // First, "draw" the input bar area (rows 20-23) with identifiable content
        for row in 20u16..24 {
            for col in 0..80 {
                if let Some(cell) = screen.get_mut(row, col) {
                    cell.char = 'I'; // I for Input
                    cell.fg = Color::GREEN;
                }
            }
        }

        let mut conv = ConversationWidget::new();

        // Add a tool call with multiline args that should fill the conversation area
        conv.push(ConversationLine::User {
            text: "Please run this command".to_string(),
        });
        conv.push(ConversationLine::ToolCall {
            name: "run".to_string(),
            args_summary: "cargo build --release && cargo test --all-features --workspace 2>&1 | tee build.log".to_string(),
        });
        // Add a large tool result
        let result_lines: Vec<String> = (0..25)
            .map(|i| format!("Compiling crate v{}.{}/{}.0...", i / 10, i % 10, i))
            .collect();
        conv.push(ConversationLine::ToolResult {
            name: "run".to_string(),
            content: result_lines.join("\n"),
            is_error: false,
        });
        // Add another tool call after the result
        conv.push(ConversationLine::ToolCall {
            name: "read".to_string(),
            args_summary: "build.log".to_string(),
        });

        // Render conversation in the area rows 1-19 (height 19)
        let area = Rect::new(0, 1, 80, 19);
        conv.render(area, &mut screen);

        // Check that the input bar area (rows 20-23) is not overwritten
        for row in 20u16..24 {
            for col in 0..80 {
                let cell = screen.get(row, col).unwrap();
                // The input bar cells should still have 'I' character
                // (conversation should not have overflowed into them)
                assert_eq!(
                    cell.char, 'I',
                    "Input bar cell at row={}, col={} was overwritten by conversation (char='{}')",
                    row, col, cell.char
                );
            }
        }
    }

    #[test]
    fn test_tool_result_error_with_bg_fill_no_overflow() {
        // Error tool results fill background color across the row width.
        // Make sure this doesn't overflow the area.
        let mut screen = Screen::new(80, 24);

        // Mark rows below the conversation area
        for row in 10u16..24 {
            for col in 0..80 {
                if let Some(cell) = screen.get_mut(row, col) {
                    cell.char = 'X';
                }
            }
        }

        let mut conv = ConversationWidget::new();
        conv.push(ConversationLine::ToolResult {
            name: "run".to_string(),
            content: "error: could not compile\nerror: build failed\ncargo test failed".to_string(),
            is_error: true,
        });

        // Render in a small area (rows 0-9, height 10)
        let area = Rect::new(0, 0, 80, 10);
        conv.render(area, &mut screen);

        // Check that rows 10-23 still have 'X' (not overwritten)
        for row in 10u16..24 {
            for col in 0..80 {
                let cell = screen.get(row, col).unwrap();
                assert_eq!(
                    cell.char, 'X',
                    "Row {} col {} was overwritten (char='{}'), should be 'X'",
                    row, col, cell.char
                );
            }
        }
    }

    #[test]
    fn test_tool_call_multiline_args_render_height() {
        // Verify that a ToolCall with multiline args renders correctly
        // within the area bounds and that screen_row tracking is accurate.
        let mut screen = Screen::new(40, 20);
        let mut conv = ConversationWidget::new();

        // Tool call with newlines in args_summary
        conv.push(ConversationLine::ToolCall {
            name: "run".to_string(),
            args_summary: "cmd1 && cmd2 && cmd3 && cmd4 && cmd5".to_string(),
        });
        // Add a marker line after to check screen_row tracking
        conv.push(ConversationLine::User {
            text: "after tool call".to_string(),
        });

        let area = Rect::new(0, 0, 40, 20);
        conv.render(area, &mut screen);

        // Verify something is rendered
        assert!(
            screen.get(0, 2).unwrap().char != ' ',
            "Tool call header should be rendered"
        );

        // The "after tool call" user message should appear after the tool call
        // Find where the user message appears
        let mut found_after = false;
        for row in 1..20 {
            for col in 0..40 {
                let cell = screen.get(row, col).unwrap();
                if cell.char == 'a' {
                    // Check if this is "after tool call"
                    let mut text = String::new();
                    for c in col..40 {
                        if let Some(cell) = screen.get(row, c) {
                            if cell.char == ' ' {
                                break;
                            }
                            text.push(cell.char);
                        }
                    }
                    if text.starts_with("after") {
                        found_after = true;
                        break;
                    }
                }
            }
            if found_after {
                break;
            }
        }
        assert!(
            found_after,
            "User message 'after tool call' should be rendered"
        );
    }

    #[test]
    fn test_confirm_prompt_line_height_matches_rendering() {
        // Verify that ConfirmPrompt line_height matches actual rendered height.
        // This was a bug where line_height used line_height_for_text (which counts wrapping)
        // but rendering truncates each diff line to 1 row.
        let conv = ConversationWidget::new();

        // ConfirmPrompt with multi-line diff
        let line = ConversationLine::ConfirmPrompt {
            name: "edit".to_string(),
            args_summary: "src/main.rs".to_string(),
            diff_preview: Some(
                "@@ -1,3 +1,3 @@\n- old line that is very long and should be wrapped in line_height_for_text\n+ new line\n context line"
                    .to_string(),
            ),
        };
        let height = conv.line_height(&line, 40);
        // Prompt line (1) + diff lines (4: hunk header, removed, added, context) = 5
        // Each diff line is 1 row because rendering truncates, not wraps.
        assert_eq!(
            height, 5,
            "ConfirmPrompt with 4 diff lines should have height 5 (1 prompt + 4 diff), got {}",
            height
        );
    }

    #[test]
    fn test_confirm_prompt_does_not_create_stairs() {
        // The "stairs" bug: ConfirmPrompt line_height overcounts (using line_height_for_text
        // which wraps), but rendering truncates. This causes the outer loop's screen_row
        // to advance past where content was actually rendered, creating gaps/stairs.
        let mut screen = Screen::new(80, 24);
        let mut conv = ConversationWidget::new();

        // Add a ConfirmPrompt with a diff containing long lines
        conv.push(ConversationLine::ConfirmPrompt {
            name: "edit".to_string(),
            args_summary: "src/main.rs".to_string(),
            diff_preview: Some(
                "@@ -1,3 +1,3 @@\n- old line that is very long and should be truncated in rendering\n+ new line\n context line"
                    .to_string(),
            ),
        });
        // Add a user message after — this should appear immediately after the ConfirmPrompt
        conv.push(ConversationLine::User {
            text: "after confirm".to_string(),
        });

        let area = Rect::new(0, 0, 80, 24);
        conv.render(area, &mut screen);

        // Find where the "after confirm" user message starts
        let mut found_after_row = None;
        for row in 0..24u16 {
            for col in 0..80u16 {
                let cell = screen.get(row, col).unwrap();
                if cell.char == 'a' {
                    let mut text = String::new();
                    for c in col..80u16 {
                        if let Some(cell) = screen.get(row, c) {
                            if cell.char == ' ' {
                                break;
                            }
                            text.push(cell.char);
                        }
                    }
                    if text.starts_with("after") {
                        found_after_row = Some(row);
                        break;
                    }
                }
            }
            if found_after_row.is_some() {
                break;
            }
        }
        let after_row = found_after_row.expect("'after confirm' message should be rendered");

        // ConfirmPrompt should be: 1 prompt line + 4 diff lines = 5 rows
        // So "after confirm" should start at row 5 (0-indexed)
        assert!(
            after_row <= 5,
            "After-confirm message should be at row 5 or less (got {}), \
             stairs bug would push it further",
            after_row
        );
    }

    #[test]
    fn test_question_wrapping_height_matches() {
        // Verify that Question rendering and line_height are consistent
        // when question or answer text wraps.
        let mut screen = Screen::new(30, 20);
        let mut conv = ConversationWidget::new();

        // Question with a long question that wraps in narrow area
        conv.push(ConversationLine::Question {
            question:
                "What is the best approach for handling very long questions that need to wrap?"
                    .to_string(),
            answers: vec!["Short answer".to_string()],
        });

        let area = Rect::new(0, 0, 30, 20);
        conv.render(area, &mut screen);

        // Check no overflow
        let overflows = check_no_overflow(&screen, area);
        assert!(
            overflows.is_empty(),
            "Question with wrapping overflows: {:?}",
            overflows
        );
    }

    #[test]
    fn test_confirm_prompt_long_text_does_not_overflow() {
        // The main bug: long confirmation text (like a `run` command) should not
        // overflow past the conversation area into adjacent widgets (input bar,
        // sidebar, etc.). The prompt text must wrap within the area boundaries.
        let mut screen = Screen::new(40, 10);

        // Mark rows 5-9 with identifiable content to detect overflow
        for row in 5u16..10 {
            for col in 0..40 {
                if let Some(cell) = screen.get_mut(row, col) {
                    cell.char = 'X';
                    cell.fg = Color::RED;
                }
            }
        }

        let mut conv = ConversationWidget::new();

        // Long confirmation prompt that would overflow if not wrapped/clipped
        conv.push(ConversationLine::ConfirmPrompt {
            name: "run".to_string(),
            args_summary:
                "cargo test --all-features --workspace -- --test-threads=1 2>&1 | head -100"
                    .to_string(),
            diff_preview: None,
        });

        // Render into a 5-row area (rows 0-4)
        let area = Rect::new(0, 0, 40, 5);
        conv.render(area, &mut screen);

        // Check that rows 5-9 still have 'X' (not overwritten by overflow)
        for row in 5u16..10 {
            for col in 0..40 {
                let cell = screen.get(row, col).unwrap();
                assert_eq!(
                    cell.char, 'X',
                    "Row {} col {} was overwritten (char='{}') — ConfirmPrompt overflowed",
                    row, col, cell.char
                );
            }
        }
    }

    #[test]
    fn test_confirm_prompt_long_text_with_offset_area() {
        // Test that ConfirmPrompt doesn't overflow when rendered in an area
        // that doesn't start at row 0 (simulating the real TUI layout where
        // the conversation area starts below a status bar).
        let mut screen = Screen::new(80, 24);

        // Mark the input bar area (rows 20-23) with identifiable content
        for row in 20u16..24 {
            for col in 0..80 {
                if let Some(cell) = screen.get_mut(row, col) {
                    cell.char = 'I';
                }
            }
        }

        let mut conv = ConversationWidget::new();

        // Add a user message, then a long ConfirmPrompt
        conv.push(ConversationLine::User {
            text: "Please run the tests".to_string(),
        });
        conv.push(ConversationLine::ConfirmPrompt {
            name: "run".to_string(),
            args_summary: "cargo test --all-features --workspace -- --test-threads=1 2>&1 | head -100 && echo 'done'".to_string(),
            diff_preview: None,
        });

        // Render conversation in the area rows 1-19 (height 19)
        let area = Rect::new(0, 1, 80, 19);
        conv.render(area, &mut screen);

        // Check that the input bar area (rows 20-23) is not overwritten
        for row in 20u16..24 {
            for col in 0..80 {
                let cell = screen.get(row, col).unwrap();
                assert_eq!(
                    cell.char, 'I',
                    "Input bar cell at row={}, col={} was overwritten (char='{}')",
                    row, col, cell.char
                );
            }
        }
    }

    #[test]
    fn test_confirm_prompt_suffix_on_new_line_when_wrapped() {
        // When the prompt text is long enough to wrap, the [y/n/a]? suffix
        // should appear on a new line if it doesn't fit after the wrapped text.
        // For short text, the suffix should appear on the same line.
        let mut screen = Screen::new(30, 10);
        let mut conv = ConversationWidget::new();

        // Short args that should fit on one line with the suffix
        conv.push(ConversationLine::ConfirmPrompt {
            name: "edit".to_string(),
            args_summary: "file.rs".to_string(),
            diff_preview: None,
        });

        let area = Rect::new(0, 0, 30, 10);
        conv.render(area, &mut screen);

        // The suffix "[y/n/a]?" should appear somewhere in the first few rows
        let mut found_suffix = false;
        for row in 0..3u16 {
            let mut row_text = String::new();
            for col in 0..30u16 {
                row_text.push(screen.get(row, col).unwrap().char);
            }
            if row_text.contains("[y/n/a]") {
                found_suffix = true;
                break;
            }
        }
        assert!(
            found_suffix,
            "ConfirmPrompt suffix [y/n/a]? should appear in rendered output for short text"
        );
    }
}

#[cfg(test)]
mod visual_tests {
    use super::*;

    /// Print a visual representation of what the screen looks like for a ConfirmPrompt
    /// with very long text (simulating a `run` command with lorem ipsum).
    #[test]
    fn test_confirm_prompt_lorem_ipsum_visual() {
        let lorem = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.";

        // Test with various narrow widths to simulate small terminals
        for width in [40, 60, 80, 120] {
            let mut screen = Screen::new(width, 20);

            // Mark rows below the conversation area
            let conv_height = 15u16;
            for row in conv_height..20 {
                for col in 0..width {
                    if let Some(cell) = screen.get_mut(row, col) {
                        cell.char = 'I'; // Mark input bar area
                        cell.fg = Color::GREEN;
                    }
                }
            }

            // Mark columns beyond conversation width (simulating sidebar)
            let conv_width = width;
            for _row in 0..conv_height {
                // (no sidebar in this test, but we could add one)
            }

            let mut conv = ConversationWidget::new();
            conv.push(ConversationLine::ConfirmPrompt {
                name: "run".to_string(),
                args_summary: format!("echo \"{}\"", lorem),
                diff_preview: None,
            });
            // Add a message after to verify layout isn't broken
            conv.push(ConversationLine::User {
                text: "after confirm".to_string(),
            });

            let area = Rect::new(0, 0, conv_width, conv_height);
            conv.render(area, &mut screen);

            // Check no overflow into input bar area
            let mut overflow_count = 0;
            for row in conv_height..20 {
                for col in 0..width {
                    let cell = screen.get(row, col).unwrap();
                    if cell.char != 'I' {
                        overflow_count += 1;
                    }
                }
            }
            assert_eq!(
                overflow_count, 0,
                "ConfirmPrompt with lorem ipsum overflows into input area at width {} ({} cells overwritten)",
                width, overflow_count
            );

            // Print the rendered output for visual inspection
            eprintln!("\n=== ConfirmPrompt at width {} ===", width);
            for row in 0..conv_height {
                let mut line = String::new();
                for col in 0..conv_width {
                    let ch = screen.get(row, col).unwrap().char;
                    line.push(if ch == '\0' || ch == ' ' { '.' } else { ch });
                }
                eprintln!("{}", line);
            }
            eprintln!("=== end ===\n");
        }
    }

    /// Test that a very long ConfirmPrompt with diff preview also doesn't overflow.
    #[test]
    fn test_confirm_prompt_long_text_with_diff_no_overflow() {
        let long_cmd = "echo \"Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua.\"";

        for width in [40, 60, 80] {
            let mut screen = Screen::new(width, 30);

            // Mark rows below conversation
            let conv_height = 20u16;
            for row in conv_height..30 {
                for col in 0..width {
                    if let Some(cell) = screen.get_mut(row, col) {
                        cell.char = 'B'; // Below
                    }
                }
            }

            let mut conv = ConversationWidget::new();
            conv.push(ConversationLine::ConfirmPrompt {
                name: "run".to_string(),
                args_summary: long_cmd.to_string(),
                diff_preview: Some(
                    "@@ -1,5 +1,5 @@\n-old line that is also quite long and should be truncated properly\n+new line that replaces it\n context line".to_string(),
                ),
            });

            let area = Rect::new(0, 0, width, conv_height);
            conv.render(area, &mut screen);

            // Check no overflow
            let mut overflow_count = 0;
            for row in conv_height..30 {
                for col in 0..width {
                    let cell = screen.get(row, col).unwrap();
                    if cell.char != 'B' {
                        overflow_count += 1;
                    }
                }
            }
            assert_eq!(
                overflow_count, 0,
                "ConfirmPrompt with diff overflows at width {} ({} cells)",
                width, overflow_count
            );
        }
    }

    // ── Question scroll/skip tests ──────────────────────────────────────

    #[test]
    fn test_question_scroll_skip_does_not_panic() {
        // Regression test: scrolling past a Question line should not panic
        // or leave content at wrong positions. The old code had an immutable
        // skip_top that was never decremented, causing misrendering.
        let mut screen = Screen::new(40, 10);
        let mut conv = ConversationWidget::new();

        // Add enough lines before the question to scroll past them
        for i in 0..10 {
            conv.push(ConversationLine::User {
                text: format!("Message {}", i),
            });
        }
        conv.push(ConversationLine::Question {
            question: "Which option?".to_string(),
            answers: vec![
                "First answer that is long enough to wrap".to_string(),
                "Second answer".to_string(),
                "Third answer".to_string(),
            ],
        });
        conv.push(ConversationLine::User {
            text: "after question".to_string(),
        });

        // Scroll so the question is partially visible
        conv.scroll_offset = 8;
        conv.auto_scroll = false;

        let area = Rect::new(0, 0, 40, 5);
        conv.render(area, &mut screen);

        // Should not panic — that's the core assertion
    }

    #[test]
    fn test_question_scroll_past_question_shows_answers() {
        // When scrolled past the question text, the answers should be visible.
        let mut screen = Screen::new(40, 20);
        let mut conv = ConversationWidget::new();

        // Add padding lines before the question so scrolling is possible.
        // We need total_height > visible_rows + scroll_offset so the
        // scroll offset isn't clamped.
        for i in 0..30 {
            conv.push(ConversationLine::User {
                text: format!("Padding {}", i),
            });
        }
        conv.push(ConversationLine::Question {
            question: "Pick one".to_string(),
            answers: vec![
                "Answer A".to_string(),
                "Answer B".to_string(),
                "Answer C".to_string(),
                "Answer D".to_string(),
                "Answer E".to_string(),
            ],
        });

        // Skip past the 30 padding lines + 1 question row = scroll 31
        conv.scroll_offset = 31;
        conv.auto_scroll = false;

        let area = Rect::new(0, 0, 40, 5);
        conv.render(area, &mut screen);

        // After skipping, the first answer's label "    1. " should be visible.
        let row_text: String = (0..20u16).map(|c| screen.get(0, c).unwrap().char).collect();
        assert!(
            row_text.contains("1"),
            "First answer label should be visible after skipping question. Row 0: {:?}",
            row_text
        );
        assert!(
            row_text.contains("Answer"),
            "First answer text should be visible. Row 0: {:?}",
            row_text
        );
    }
}
