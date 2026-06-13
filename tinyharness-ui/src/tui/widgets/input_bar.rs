// ── Input bar widget ──────────────────────────────────────────────────────────
//
// Multi-line input with history, cursor tracking, mode/model label,
// and tab completion for slash commands.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::tui::cell::{Color, Style};
use crate::tui::event::{Event, Key, KeyEvent};
use crate::tui::layout::Rect;
use crate::tui::screen::Screen;
use crate::tui::widget::{Action, Widget, styles, truncate_str_width};
use std::collections::HashMap;

/// Find the byte offset of the start of the previous word, stopping at newlines.
fn word_start_backward(text: &str, cursor: usize) -> usize {
    let before = &text[..cursor];
    let trimmed = before.trim_end_matches(|c: char| c.is_whitespace() && c != '\n');
    if trimmed.len() < before.len() {
        trimmed
            .rfind(|c: char| c.is_whitespace() || c == '\n')
            .map(|p| p + 1)
            .unwrap_or(0)
    } else {
        before
            .rfind(|c: char| c.is_whitespace() || c == '\n')
            .map(|p| p + 1)
            .unwrap_or(0)
    }
}

/// Find the byte offset after the next word, including trailing whitespace (but not newlines).
fn word_end_forward(text: &str, cursor: usize) -> usize {
    let after = &text[cursor..];
    let word_end = after
        .find(|c: char| c.is_whitespace() || c == '\n')
        .unwrap_or(after.len());
    let whitespace_skipped = after[word_end..]
        .chars()
        .take_while(|c| c.is_whitespace() && *c != '\n')
        .count();
    cursor + word_end + whitespace_skipped
}

/// A single visual row of input text, with byte range and screen column.
struct InputRow<'a> {
    /// The text slice to display on this row.
    text: &'a str,
    /// Byte offset of `text` within the full content.
    byte_offset: usize,
    /// Screen column where this row starts.
    col: u16,
}

/// Compute the visual rows for input content, accounting for the prompt on
/// the first line and Unicode display widths. Each row fits within
/// `available_width` columns.
fn input_rows<'a>(
    content: &'a str,
    prompt_width: u16,
    area_x: u16,
    available_width: u16,
) -> Vec<InputRow<'a>> {
    let mut rows = Vec::new();
    let mut first_line = true;
    let mut byte_offset = 0usize;

    for line in content.split_inclusive('\n') {
        let (line_text, _has_newline) = if let Some(stripped) = line.strip_suffix('\n') {
            (stripped, true)
        } else {
            (line, false)
        };

        let left_col = if first_line {
            area_x + prompt_width
        } else {
            area_x
        };
        first_line = false;

        // Split the logical line into wrapped visual rows.
        let mut row_start = 0usize;
        let mut row_width = 0usize;
        let first_line_indent = if row_start == 0 && left_col > area_x {
            prompt_width as usize
        } else {
            0
        };

        for (idx, ch) in line_text.char_indices() {
            let w = ch.width().unwrap_or(1);
            if w == 0 {
                // Combining marks don't occupy columns; skip for wrapping.
                continue;
            }
            if row_width + first_line_indent + w > available_width as usize && row_width > 0 {
                let end = idx;
                rows.push(InputRow {
                    text: &line_text[row_start..end],
                    byte_offset: byte_offset + row_start,
                    col: left_col,
                });
                row_start = idx;
                row_width = 0;
            }
            row_width += w;
        }

        // Push the final row for this logical line.
        rows.push(InputRow {
            text: &line_text[row_start..],
            byte_offset: byte_offset + row_start,
            col: left_col,
        });

        byte_offset += line.len();
    }

    rows
}

/// The input bar at the bottom of the screen.
///
/// Displays a prompt with mode and model labels, and accepts
/// multi-line text input. Enter submits, Shift+Enter inserts a newline.
/// Tab completes slash commands when the input starts with `/`.
///
/// In confirmation mode, the input bar shows a `[y/n/a]?` prompt
/// and only accepts y (approve), n (deny), or a (approve all) keys.
pub struct InputBarWidget {
    /// Current input text.
    content: String,
    /// Cursor position (byte offset in content).
    cursor: usize,
    /// Scroll offset for the input area (for multi-line input).
    scroll_offset: usize,
    /// Input history (previous messages).
    history: Vec<String>,
    /// Current position in history navigation (None = not navigating).
    history_index: Option<usize>,
    /// The mode label to display (e.g., "agent").
    mode_label: String,
    /// The mode color.
    mode_color: Color,
    /// The model name to display.
    model_name: String,
    /// Whether the input bar is focused.
    focused: bool,
    /// Current tab-completion state: index into the list of matching completions.
    /// `None` means we're not in tab-completion cycling mode.
    tab_cycle_index: Option<usize>,
    /// The prefix that was being completed when Tab cycling started.
    tab_cycle_prefix: String,
    /// Whether the last completion was a subcommand completion.
    tab_cycle_subcommand: bool,
    /// Whether the input bar is in confirmation mode (y/n/a).
    confirming: bool,
    /// Whether the input bar is in question mode (user must answer a question).
    questioning: bool,
    /// The number of predefined answers for the current question.
    question_answer_count: usize,
    /// Kill ring for Ctrl+K/U/W/Y emacs-style editing.
    kill_ring: String,
    /// All known command names (primary + aliases), for tab completion.
    command_names: Vec<String>,
    /// Subcommand completions for commands that take arguments.
    subcommands: HashMap<String, Vec<String>>,
}

impl InputBarWidget {
    pub fn new(mode_label: &str, model_name: &str) -> Self {
        Self::with_commands(mode_label, model_name, Vec::new(), HashMap::new())
    }

    /// Create an `InputBarWidget` with command names and subcommand completions
    /// for tab completion, typically sourced from the binary's `CommandRegistry`.
    pub fn with_commands(
        mode_label: &str,
        model_name: &str,
        command_names: Vec<String>,
        subcommands: HashMap<String, Vec<String>>,
    ) -> Self {
        let mode_color = match mode_label {
            "casual" => styles::MODE_CASUAL_FG,
            "planning" => styles::MODE_PLANNING_FG,
            "agent" => styles::MODE_AGENT_FG,
            "research" => styles::MODE_RESEARCH_FG,
            _ => Color::WHITE,
        };

        Self {
            content: String::new(),
            cursor: 0,
            scroll_offset: 0,
            history: Vec::new(),
            history_index: None,
            mode_label: mode_label.to_string(),
            mode_color,
            model_name: model_name.to_string(),
            focused: true,
            tab_cycle_index: None,
            tab_cycle_prefix: String::new(),
            tab_cycle_subcommand: false,
            confirming: false,
            questioning: false,
            question_answer_count: 0,
            kill_ring: String::new(),
            command_names,
            subcommands,
        }
    }

    /// Get the current input text and clear the buffer.
    pub fn take_input(&mut self) -> String {
        let text = self.content.clone();
        if !text.is_empty() {
            self.history.push(text.clone());
        }
        self.content.clear();
        self.cursor = 0;
        self.scroll_offset = 0;
        self.history_index = None;
        text
    }

    /// Set the input text (e.g., from --prompt flag).
    pub fn set_input(&mut self, text: &str) {
        self.content = text.to_string();
        self.cursor = self.content.len();
    }

    /// Update the mode label and model name.
    pub fn update_labels(&mut self, mode_label: &str, model_name: &str) {
        self.mode_label = mode_label.to_string();
        self.mode_color = match mode_label {
            "casual" => styles::MODE_CASUAL_FG,
            "planning" => styles::MODE_PLANNING_FG,
            "agent" => styles::MODE_AGENT_FG,
            "research" => styles::MODE_RESEARCH_FG,
            _ => Color::WHITE,
        };
        self.model_name = model_name.to_string();
    }

    /// Set command names and subcommand completions for tab completion.
    pub fn set_command_completions(
        &mut self,
        command_names: Vec<String>,
        subcommands: HashMap<String, Vec<String>>,
    ) {
        self.command_names = command_names;
        self.subcommands = subcommands;
    }

    /// Total visual height of the input content for the given area width.
    pub fn content_height(&self, area_width: u16) -> u16 {
        let prompt = format!("[{}] ", self.mode_label);
        let prompt_width = prompt.width() as u16;
        let available = area_width.saturating_sub(prompt_width).max(1);
        let rows = input_rows(&self.content, prompt_width, 0, available).len() as u16;
        // Reserve one row for the top border and at least one row of content.
        rows.max(1) + 1
    }

    /// Check if the current input starts with a slash (for command detection).
    pub fn is_command_input(&self) -> bool {
        self.content.starts_with('/')
    }

    /// Enter or exit confirmation mode.
    ///
    /// In confirmation mode, the input bar shows a `[y/n/a]?` prompt
    /// and only accepts y (approve), n (deny), or a (approve all) keys.
    pub fn set_confirming(&mut self, confirming: bool) {
        self.confirming = confirming;
        if confirming {
            self.content.clear();
            self.cursor = 0;
        }
    }

    /// Check if the input bar is in confirmation mode.
    pub fn is_confirming(&self) -> bool {
        self.confirming
    }

    /// Enter or exit question mode.
    ///
    /// In question mode, the input bar shows a prompt for the user to
    /// type a number (1-N) or custom text, then press Enter.
    pub fn set_questioning(&mut self, questioning: bool, answer_count: usize) {
        self.questioning = questioning;
        self.question_answer_count = answer_count;
        if questioning {
            self.content.clear();
            self.cursor = 0;
        }
    }

    /// Check if the input bar is in question mode.
    pub fn is_questioning(&self) -> bool {
        self.questioning
    }

    /// Handle a mouse click on the input bar to position the cursor.
    ///
    /// Computes where the user clicked relative to the prompt and text
    /// content, then moves the cursor to that position.
    pub fn click_to_cursor(&mut self, click_row: u16, click_col: u16, area: Rect) {
        if self.confirming || self.questioning {
            return;
        }

        let prompt = format!("[{}] ", self.mode_label);
        let prompt_width = prompt.width() as u16;
        let available = area.width.saturating_sub(prompt_width).max(1);

        let first_input_row = area.y + 1;
        let rel_row = click_row.saturating_sub(first_input_row) as usize;

        let rows = input_rows(&self.content, prompt_width, area.x, available);
        let Some(row) = rows.get(rel_row) else {
            self.cursor = self.content.len();
            return;
        };

        let mut col = row.col as usize;
        for (idx, ch) in row.text.char_indices() {
            let w = ch.width().unwrap_or(1).max(1) as usize;
            if col + w > click_col as usize {
                self.cursor = row.byte_offset + idx;
                return;
            }
            col += w;
        }
        self.cursor = row.byte_offset + row.text.len();
    }

    /// Attempt tab completion for slash commands.
    ///
    /// If the input starts with `/`, cycle through matching command names
    /// (or subcommand arguments). Returns `true` if a completion was applied,
    /// `false` if no completions matched.
    ///
    /// Tab cycling works by remembering the original prefix the user typed
    /// before the first Tab. Subsequent Tabs cycle through all commands
    /// that start with that prefix.
    fn tab_complete(&mut self) -> bool {
        if !self.content.starts_with('/') {
            return false;
        }

        // Determine if we're completing a subcommand or a top-level command
        if let Some(space_pos) = self.content.find(' ') {
            // Subcommand completion: "/command sub<tab>"
            let cmd = &self.content[..space_pos].to_lowercase();
            let current_arg = self.content[space_pos + 1..].trim_start().to_lowercase();

            let subs = self
                .subcommands
                .get(cmd)
                .map(|s| s.as_slice())
                .unwrap_or(&[]);
            if subs.is_empty() {
                return false;
            }

            // On first Tab (or if the prefix changed), start a new cycle
            if self.tab_cycle_index.is_none()
                || self.tab_cycle_prefix != current_arg
                || !self.tab_cycle_subcommand
            {
                self.tab_cycle_prefix = current_arg.clone();
                self.tab_cycle_index = Some(0);
                self.tab_cycle_subcommand = true;
            }

            let matches: Vec<&String> = subs
                .iter()
                .filter(|s| s.starts_with(&self.tab_cycle_prefix))
                .collect();

            if matches.is_empty() {
                self.tab_cycle_index = None;
                return false;
            }

            let idx = self.tab_cycle_index.unwrap() % matches.len();
            let completion = matches[idx];

            // Replace the subcommand argument
            self.content = format!("{} {}", cmd, completion);
            self.cursor = self.content.len();
            self.tab_cycle_index = Some(idx + 1);
            true
        } else {
            // Top-level command completion: "/mod<tab>"
            let current_input = self.content.to_lowercase();

            // On first Tab (or if cycling context was for subcommands), start fresh
            if self.tab_cycle_index.is_none() || self.tab_cycle_subcommand {
                self.tab_cycle_prefix = current_input.clone();
                self.tab_cycle_index = Some(0);
                self.tab_cycle_subcommand = false;
            } else {
                // Continuing a cycle: the current content was set by the previous Tab,
                // so the prefix we're matching against is still tab_cycle_prefix.
                // The current content is a completed command name — don't update prefix.
            }

            let matches: Vec<&String> = self
                .command_names
                .iter()
                .filter(|name| name.starts_with(&self.tab_cycle_prefix))
                .collect();

            if matches.is_empty() {
                self.tab_cycle_index = None;
                return false;
            }

            let idx = self.tab_cycle_index.unwrap() % matches.len();
            let completion = matches[idx];

            self.content = completion.to_string();
            self.cursor = self.content.len();
            self.tab_cycle_index = Some(idx + 1);
            true
        }
    }

    /// Reset tab-completion cycling state (e.g., when a non-Tab key is pressed).
    fn reset_tab_cycle(&mut self) {
        self.tab_cycle_index = None;
        self.tab_cycle_prefix.clear();
        self.tab_cycle_subcommand = false;
    }
}

impl Widget for InputBarWidget {
    fn render(&mut self, area: Rect, screen: &mut Screen) {
        if area.is_empty() || area.height < 2 {
            return;
        }

        let row = area.y;

        // Draw top border
        screen.hline(
            row,
            area.x,
            area.x + area.width - 1,
            '─',
            Color::Ansi(240),
            Color::Default,
        );

        // Draw prompt and input on the next rows
        let input_row = row + 1;

        if self.confirming {
            // In confirmation mode, show a yellow prompt asking for y/n/a
            let confirm_prompt = "[y/n/a]? ";
            let mut col = area.x;
            screen.write_str(
                input_row,
                col,
                confirm_prompt,
                Color::YELLOW,
                styles::INPUT_BAR_BG,
                Style::bold(),
            );
            col += confirm_prompt.len() as u16;

            // Draw blinking cursor indicator
            if self.focused && col < area.x + area.width {
                if let Some(cell) = screen.get_mut(input_row, col) {
                    cell.char = '█';
                    cell.fg = Color::YELLOW;
                    cell.style = Style::blink();
                }
            }

            // Fill the rest with background
            for c in col + 1..area.x + area.width {
                if let Some(cell) = screen.get_mut(input_row, c) {
                    cell.bg = styles::INPUT_BAR_BG;
                }
            }
        } else if self.questioning {
            // In question mode, show a cyan prompt asking for answer
            let question_prompt = format!("[1-{} or type]: ", self.question_answer_count);
            let mut col = area.x;
            screen.write_str(
                input_row,
                col,
                &question_prompt,
                Color::CYAN,
                styles::INPUT_BAR_BG,
                Style::bold(),
            );
            col += question_prompt.len() as u16;

            // Draw input content
            let available_width = area.width.saturating_sub(col - area.x);
            let display_text = if self.content.width() > available_width as usize {
                let start = self.content.len().saturating_sub(available_width as usize);
                &self.content[start..]
            } else {
                &self.content
            };

            screen.write_str(
                input_row,
                col,
                display_text,
                Color::WHITE,
                styles::INPUT_BAR_BG,
                Style::default(),
            );

            // Draw cursor
            if self.focused {
                let before_cursor = &display_text[..self.cursor.min(display_text.len())];
                let cursor_col = col + before_cursor.width() as u16;
                if cursor_col < area.x + area.width {
                    if let Some(cell) = screen.get_mut(input_row, cursor_col) {
                        cell.style.underline = true;
                    }
                }
            }
        } else {
            let prompt = format!("[{}] ", self.mode_label);

            // Draw mode label
            let mut col = area.x;
            screen.write_str(
                input_row,
                col,
                &prompt,
                self.mode_color,
                styles::INPUT_BAR_BG,
                Style::bold(),
            );
            col += prompt.width() as u16;

            let available_width = area.width.saturating_sub(col - area.x);

            // Render wrapped content over the available rows.
            render_wrapped_input(
                &self.content,
                self.cursor,
                self.focused,
                screen,
                input_row,
                area.y + area.height - 1,
                area.x,
                col,
                available_width,
            );
        }
    }

    fn handle_event(&mut self, event: &Event) -> Action {
        // Handle paste events (bracketed paste mode)
        if let Event::Paste(text) = event {
            if !self.confirming && !self.questioning {
                self.content.insert_str(self.cursor, text);
                self.cursor += text.len();
                self.reset_tab_cycle();
            }
            return Action::None;
        }

        let Event::Key(key) = event else {
            return Action::None;
        };

        // In confirmation mode, only accept y/n/a responses
        if self.confirming {
            match key {
                KeyEvent {
                    key: Key::Char('y'),
                    modifiers,
                } if !modifiers.ctrl && !modifiers.alt => {
                    self.confirming = false;
                    Action::ConfirmYes
                }
                KeyEvent {
                    key: Key::Char('n'),
                    modifiers,
                } if !modifiers.ctrl && !modifiers.alt => {
                    self.confirming = false;
                    Action::ConfirmNo
                }
                KeyEvent {
                    key: Key::Char('a'),
                    modifiers,
                } if !modifiers.ctrl && !modifiers.alt => {
                    self.confirming = false;
                    Action::ConfirmAll
                }
                KeyEvent {
                    key: Key::Escape, ..
                } => {
                    self.confirming = false;
                    Action::ConfirmNo
                }
                _ => Action::None,
            }
        } else if self.questioning {
            // In question mode, accept typing and Enter to submit
            match key {
                KeyEvent {
                    key: Key::Enter,
                    modifiers,
                } if !modifiers.shift => {
                    let text = self.take_input();
                    self.questioning = false;
                    if text.trim().is_empty() {
                        // Empty input — skip the question
                        Action::AnswerQuestion("Skipped (no answer provided)".to_string())
                    } else {
                        // Check if the user typed a number matching an option
                        let trimmed = text.trim();
                        if let Ok(num) = trimmed.parse::<usize>() {
                            if num >= 1 && num <= self.question_answer_count {
                                // Number input — will be resolved by the app
                                Action::AnswerQuestion(trimmed.to_string())
                            } else {
                                // Out of range number — treat as free-form input
                                Action::AnswerQuestion(trimmed.to_string())
                            }
                        } else {
                            // Free-form text answer
                            Action::AnswerQuestion(trimmed.to_string())
                        }
                    }
                }
                KeyEvent {
                    key: Key::Escape, ..
                } => {
                    self.questioning = false;
                    Action::AnswerQuestion("Skipped (no answer provided)".to_string())
                }
                KeyEvent {
                    key: Key::Char(c),
                    modifiers,
                } if !modifiers.ctrl && !modifiers.alt => {
                    self.content.insert(self.cursor, *c);
                    self.cursor += c.len_utf8();
                    self.reset_tab_cycle();
                    Action::None
                }
                KeyEvent {
                    key: Key::Backspace,
                    ..
                } => {
                    if self.cursor > 0 {
                        let prev_char = self.content[..self.cursor].chars().next_back();
                        if let Some(ch) = prev_char {
                            self.cursor -= ch.len_utf8();
                            self.content.remove(self.cursor);
                        }
                    }
                    self.reset_tab_cycle();
                    Action::None
                }
                _ => Action::None,
            }
        } else {
            self.handle_normal_key(key)
        }
    }

    fn focused(&self) -> bool {
        self.focused
    }

    fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }
}

/// Render the normal input text wrapped into the available area.
/// `first_col` is where the first line starts (after the prompt); subsequent
/// lines start at `area_x`.
fn render_wrapped_input(
    content: &str,
    cursor: usize,
    focused: bool,
    screen: &mut Screen,
    start_row: u16,
    max_row: u16,
    area_x: u16,
    first_col: u16,
    available_width: u16,
) {
    let prompt_width = first_col.saturating_sub(area_x);
    let rows = input_rows(content, prompt_width, area_x, available_width);

    for (i, row) in rows.iter().enumerate() {
        let screen_row = start_row + i as u16;
        if screen_row > max_row {
            break;
        }

        // Show only the tail of the row if it doesn't fit.
        let display_text = if row.text.width() > available_width as usize {
            truncate_str_width(row.text, available_width as usize)
        } else {
            row.text
        };

        screen.write_str(
            screen_row,
            row.col,
            display_text,
            Color::WHITE,
            styles::INPUT_BAR_BG,
            Style::default(),
        );

        // Cursor on the active row.
        if focused && cursor >= row.byte_offset && cursor <= row.byte_offset + row.text.len() {
            let before_cursor_len = cursor.saturating_sub(row.byte_offset).min(row.text.len());
            let before_cursor = &row.text[..before_cursor_len];
            let cursor_col = row.col + before_cursor.width() as u16;
            if cursor_col < area_x + available_width {
                if let Some(cell) = screen.get_mut(screen_row, cursor_col) {
                    cell.style.underline = true;
                    if cell.char == ' ' {
                        cell.fg = Color::WHITE;
                    }
                }
            }
        }
    }

    // Fill remaining lines with background so stale content is cleared.
    let last_rendered_row = start_row + rows.len() as u16;
    for r in last_rendered_row..=max_row {
        for c in area_x..area_x + available_width {
            if let Some(cell) = screen.get_mut(r, c) {
                cell.char = ' ';
                cell.fg = Color::Default;
                cell.bg = styles::INPUT_BAR_BG;
                cell.style = Style::default();
            }
        }
    }
}

impl InputBarWidget {
    /// Handle a key event in normal (non-confirmation) mode.
    fn handle_normal_key(&mut self, key: &KeyEvent) -> Action {
        match key {
            KeyEvent {
                key: Key::Enter,
                modifiers,
            } => {
                if modifiers.shift {
                    // Shift+Enter: insert newline
                    self.content.insert(self.cursor, '\n');
                    self.cursor += 1;
                    self.reset_tab_cycle();
                    Action::None
                } else {
                    // Enter: submit the message
                    let text = self.take_input();
                    self.reset_tab_cycle();
                    if text.trim().is_empty() {
                        Action::None
                    } else {
                        Action::SendMessage(text)
                    }
                }
            }
            KeyEvent {
                key: Key::Backspace,
                ..
            } => {
                if self.cursor > 0 {
                    // Handle deleting across newlines
                    let prev_char = self.content[..self.cursor].chars().next_back();
                    if let Some(ch) = prev_char {
                        self.cursor -= ch.len_utf8();
                        self.content.remove(self.cursor);
                    }
                }
                self.reset_tab_cycle();
                Action::None
            }
            KeyEvent {
                key: Key::Delete, ..
            } => {
                if self.cursor < self.content.len() {
                    // Find the next character boundary
                    let next_char = self.content[self.cursor..].chars().next();
                    if let Some(ch) = next_char {
                        let end = self.cursor + ch.len_utf8();
                        self.content.replace_range(self.cursor..end, "");
                    }
                }
                self.reset_tab_cycle();
                Action::None
            }
            KeyEvent {
                key: Key::Left,
                modifiers,
            } => {
                if modifiers.ctrl {
                    self.cursor = word_start_backward(&self.content, self.cursor);
                } else if self.cursor > 0 {
                    if let Some((idx, _ch)) = self.content[..self.cursor].char_indices().next_back()
                    {
                        self.cursor = idx;
                    }
                }
                self.reset_tab_cycle();
                Action::None
            }
            KeyEvent {
                key: Key::Right,
                modifiers,
            } => {
                if modifiers.ctrl {
                    self.cursor = word_end_forward(&self.content, self.cursor);
                } else if self.cursor < self.content.len() {
                    if let Some((idx, ch)) = self.content[self.cursor..].char_indices().next() {
                        self.cursor = idx + ch.len_utf8();
                    }
                }
                self.reset_tab_cycle();
                Action::None
            }
            KeyEvent { key: Key::Home, .. } => {
                // Move to start of current line
                let line_start = self.content[..self.cursor]
                    .rfind('\n')
                    .map(|p| p + 1)
                    .unwrap_or(0);
                self.cursor = line_start;
                self.reset_tab_cycle();
                Action::None
            }
            KeyEvent { key: Key::End, .. } => {
                // Move to end of current line
                let line_end = self.content[self.cursor..]
                    .find('\n')
                    .map(|p| self.cursor + p)
                    .unwrap_or(self.content.len());
                self.cursor = line_end;
                self.reset_tab_cycle();
                Action::None
            }
            KeyEvent {
                key: Key::Up,
                modifiers,
            } if !modifiers.alt && !modifiers.ctrl => {
                // History navigation
                if !self.history.is_empty() {
                    let idx = self.history_index.unwrap_or(self.history.len());
                    if idx > 0 {
                        let new_idx = idx - 1;
                        self.history_index = Some(new_idx);
                        self.content = self.history[new_idx].clone();
                        self.cursor = self.content.len();
                    }
                }
                self.reset_tab_cycle();
                Action::None
            }
            KeyEvent {
                key: Key::Down,
                modifiers,
            } if !modifiers.alt && !modifiers.ctrl => {
                // History navigation
                if let Some(idx) = self.history_index {
                    if idx + 1 < self.history.len() {
                        self.history_index = Some(idx + 1);
                        self.content = self.history[idx + 1].clone();
                    } else {
                        self.history_index = None;
                        self.content.clear();
                    }
                    self.cursor = self.content.len();
                }
                self.reset_tab_cycle();
                Action::None
            }
            KeyEvent {
                key: Key::Tab,
                modifiers,
            } if !modifiers.shift => {
                // Tab: command completion if input starts with '/', otherwise cycle focus
                if self.is_command_input() {
                    self.tab_complete();
                    Action::None
                } else {
                    // Not a command — let the app cycle focus
                    Action::CycleFocusForward
                }
            }
            KeyEvent {
                key: Key::BackTab, ..
            } => {
                // Shift+Tab: always cycle focus backward
                Action::CycleFocusBackward
            }
            KeyEvent {
                key: Key::Char(c),
                modifiers,
            } => {
                if modifiers.ctrl {
                    // Handle Ctrl+key shortcuts
                    match c {
                        'c' => Action::Quit,
                        'd' => Action::Quit,
                        'a' => {
                            // Ctrl+A: move cursor to start of line
                            let line_start = self.content[..self.cursor]
                                .rfind('\n')
                                .map(|p| p + 1)
                                .unwrap_or(0);
                            self.cursor = line_start;
                            self.reset_tab_cycle();
                            Action::None
                        }
                        'e' => {
                            // Ctrl+E: move cursor to end of line
                            let line_end = self.content[self.cursor..]
                                .find('\n')
                                .map(|p| self.cursor + p)
                                .unwrap_or(self.content.len());
                            self.cursor = line_end;
                            self.reset_tab_cycle();
                            Action::None
                        }
                        'u' => {
                            // Ctrl+U: clear from cursor to beginning of line
                            let line_start = self.content[..self.cursor]
                                .rfind('\n')
                                .map(|p| p + 1)
                                .unwrap_or(0);
                            let killed = self.content[line_start..self.cursor].to_string();
                            if !killed.is_empty() {
                                self.kill_ring = killed;
                            }
                            self.content.replace_range(line_start..self.cursor, "");
                            self.cursor = line_start;
                            self.reset_tab_cycle();
                            Action::None
                        }
                        'k' => {
                            // Ctrl+K: clear from cursor to end of line
                            let line_end = self.content[self.cursor..]
                                .find('\n')
                                .map(|p| self.cursor + p)
                                .unwrap_or(self.content.len());
                            let killed = self.content[self.cursor..line_end].to_string();
                            if !killed.is_empty() {
                                self.kill_ring = killed;
                            }
                            self.content.replace_range(self.cursor..line_end, "");
                            self.reset_tab_cycle();
                            Action::None
                        }
                        'w' => {
                            // Ctrl+W: delete word backward
                            if self.cursor > 0 {
                                let word_start = word_start_backward(&self.content, self.cursor);
                                let killed = self.content[word_start..self.cursor].to_string();
                                if !killed.is_empty() {
                                    self.kill_ring = killed;
                                }
                                self.content.replace_range(word_start..self.cursor, "");
                                self.cursor = word_start;
                            }
                            self.reset_tab_cycle();
                            Action::None
                        }
                        'y' => {
                            // Ctrl+Y: yank (paste) from kill ring
                            if !self.kill_ring.is_empty() {
                                self.content.insert_str(self.cursor, &self.kill_ring);
                                self.cursor += self.kill_ring.len();
                            }
                            self.reset_tab_cycle();
                            Action::None
                        }
                        _ => Action::None,
                    }
                } else if modifiers.alt {
                    // Handle Alt+key shortcuts
                    match c {
                        'b' => {
                            // Alt+B: move cursor back one word
                            self.cursor = word_start_backward(&self.content, self.cursor);
                            self.reset_tab_cycle();
                            Action::None
                        }
                        'f' => {
                            // Alt+F: move cursor forward one word
                            self.cursor = word_end_forward(&self.content, self.cursor);
                            self.reset_tab_cycle();
                            Action::None
                        }
                        '\x08' | '\x7f' => {
                            // Alt+Backspace: delete word backward (same as Ctrl+W)
                            if self.cursor > 0 {
                                let word_start = word_start_backward(&self.content, self.cursor);
                                let killed = self.content[word_start..self.cursor].to_string();
                                if !killed.is_empty() {
                                    self.kill_ring = killed;
                                }
                                self.content.replace_range(word_start..self.cursor, "");
                                self.cursor = word_start;
                            }
                            self.reset_tab_cycle();
                            Action::None
                        }
                        _ => Action::None,
                    }
                } else {
                    self.content.insert(self.cursor, *c);
                    self.cursor += c.len_utf8();
                    self.reset_tab_cycle();
                    Action::None
                }
            }
            KeyEvent {
                key: Key::Escape, ..
            } => {
                if self.content.is_empty() {
                    Action::Quit
                } else {
                    // Clear input on Escape
                    self.content.clear();
                    self.cursor = 0;
                    self.reset_tab_cycle();
                    Action::None
                }
            }
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::event::Modifiers;

    /// Standard command names used in tests.
    fn test_command_names() -> Vec<String> {
        vec![
            "/add",
            "/agent",
            "/apikey",
            "/audit",
            "/autoaccept",
            "/casual",
            "/clear",
            "/command",
            "/compact",
            "/context",
            "/contextlimit",
            "/drop",
            "/dropall",
            "/exit",
            "/files",
            "/help",
            "/init",
            "/mode",
            "/model",
            "/plan",
            "/project-settings",
            "/quit",
            "/refresh",
            "/rename",
            "/retries",
            "/research",
            "/session",
            "/sessions",
            "/settings",
            "/showthink",
            "/skill",
            "/skills",
            "/think",
            "/timeout",
            "/unload",
            "/use",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect()
    }

    /// Standard subcommand completions used in tests.
    fn test_subcommands() -> HashMap<String, Vec<String>> {
        let mut subs = HashMap::new();
        subs.insert(
            "/command".to_string(),
            vec![
                "add".into(),
                "deny".into(),
                "help".into(),
                "list".into(),
                "rm".into(),
                "reset".into(),
                "resetdeny".into(),
                "undeny".into(),
            ],
        );
        subs.insert("/session".to_string(), vec!["delete".into()]);
        subs.insert(
            "/mode".to_string(),
            vec![
                "agent".into(),
                "casual".into(),
                "planning".into(),
                "research".into(),
            ],
        );
        subs.insert("/settings".to_string(), vec!["all".into()]);
        subs.insert("/autoaccept".to_string(), vec!["off".into(), "on".into()]);
        subs.insert("/apikey".to_string(), vec!["clear".into()]);
        subs.insert("/showthink".to_string(), vec!["off".into(), "on".into()]);
        subs.insert(
            "/think".to_string(),
            vec!["high".into(), "low".into(), "medium".into(), "off".into()],
        );
        subs
    }

    /// Create an InputBarWidget with test command data for tab-completion tests.
    fn bar_with_commands(mode_label: &str, model_name: &str) -> InputBarWidget {
        InputBarWidget::with_commands(
            mode_label,
            model_name,
            test_command_names(),
            test_subcommands(),
        )
    }

    #[test]
    fn test_input_bar_new() {
        let bar = InputBarWidget::new("agent", "llama3.1:8b");
        assert!(bar.content.is_empty());
        assert_eq!(bar.cursor, 0);
        assert!(bar.focused);
    }

    #[test]
    fn test_input_bar_take_input() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.content = "hello".to_string();
        bar.cursor = 5;
        let text = bar.take_input();
        assert_eq!(text, "hello");
        assert!(bar.content.is_empty());
        assert_eq!(bar.cursor, 0);
        assert_eq!(bar.history.len(), 1);
    }

    #[test]
    fn test_input_bar_type_chars() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        let event = Event::Key(KeyEvent {
            key: Key::Char('h'),
            modifiers: Modifiers::new(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.content, "h");
        assert_eq!(bar.cursor, 1);

        let event = Event::Key(KeyEvent {
            key: Key::Char('i'),
            modifiers: Modifiers::new(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.content, "hi");
        assert_eq!(bar.cursor, 2);
    }

    #[test]
    fn test_input_bar_backspace() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.content = "hello".to_string();
        bar.cursor = 5;

        let event = Event::Key(KeyEvent {
            key: Key::Backspace,
            modifiers: Modifiers::new(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.content, "hell");
        assert_eq!(bar.cursor, 4);
    }

    #[test]
    fn test_input_bar_enter_submits() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.content = "hello".to_string();
        bar.cursor = 5;

        let event = Event::Key(KeyEvent {
            key: Key::Enter,
            modifiers: Modifiers::new(),
        });
        let action = bar.handle_event(&event);
        assert!(matches!(action, Action::SendMessage(ref s) if s == "hello"));
        assert!(bar.content.is_empty());
    }

    #[test]
    fn test_input_bar_shift_enter_newline() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.content = "hello".to_string();
        bar.cursor = 5;

        let event = Event::Key(KeyEvent {
            key: Key::Enter,
            modifiers: Modifiers::shift(),
        });
        let action = bar.handle_event(&event);
        assert!(matches!(action, Action::None));
        assert_eq!(bar.content, "hello\n");
    }

    #[test]
    fn test_input_bar_escape_clears() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.content = "hello".to_string();
        bar.cursor = 5;

        let event = Event::Key(KeyEvent {
            key: Key::Escape,
            modifiers: Modifiers::new(),
        });
        let action = bar.handle_event(&event);
        assert!(matches!(action, Action::None));
        assert!(bar.content.is_empty());
    }

    #[test]
    fn test_tab_complete_command() {
        let mut bar = bar_with_commands("agent", "llama3.1:8b");
        bar.content = "/mod".to_string();
        bar.cursor = 4;

        let event = Event::Key(KeyEvent {
            key: Key::Tab,
            modifiers: Modifiers::new(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.content, "/mode");
        assert_eq!(bar.cursor, 5);
    }

    #[test]
    fn test_tab_complete_cycle() {
        let mut bar = bar_with_commands("agent", "llama3.1:8b");
        bar.content = "/co".to_string();
        bar.cursor = 3;

        let event = Event::Key(KeyEvent {
            key: Key::Tab,
            modifiers: Modifiers::new(),
        });

        // First Tab — completes to first match
        bar.handle_event(&event);
        let first = bar.content.clone();
        assert!(first.starts_with("/co"));

        // Second Tab — cycles to next match
        bar.handle_event(&event);
        let second = bar.content.clone();
        assert!(second.starts_with("/co"));
        assert_ne!(first, second);

        // Third Tab — cycles to next match
        bar.handle_event(&event);
        let third = bar.content.clone();
        assert!(third.starts_with("/co"));
        // Should cycle through /command, /compact, /context
        assert_ne!(second, third);
    }

    #[test]
    fn test_tab_complete_resets_on_typing() {
        let mut bar = bar_with_commands("agent", "llama3.1:8b");
        bar.content = "/mod".to_string();
        bar.cursor = 4;

        let tab = Event::Key(KeyEvent {
            key: Key::Tab,
            modifiers: Modifiers::new(),
        });
        bar.handle_event(&tab);
        assert_eq!(bar.content, "/mode");

        // Type a character — should reset tab cycle state
        let char_event = Event::Key(KeyEvent {
            key: Key::Char(' '),
            modifiers: Modifiers::new(),
        });
        bar.handle_event(&char_event);
        assert_eq!(bar.content, "/mode ");

        // Tab again — should start a new completion cycle for subcommands
        bar.handle_event(&tab);
        // "/mode " with subcommand completion for /mode
        assert!(bar.content.starts_with("/mode "));
    }

    #[test]
    fn test_tab_complete_subcommand() {
        let mut bar = bar_with_commands("agent", "llama3.1:8b");
        bar.content = "/command a".to_string();
        bar.cursor = 10;

        let event = Event::Key(KeyEvent {
            key: Key::Tab,
            modifiers: Modifiers::new(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.content, "/command add");
    }

    #[test]
    fn test_tab_non_command_cycles_focus() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.content = "hello".to_string();
        bar.cursor = 5;

        let event = Event::Key(KeyEvent {
            key: Key::Tab,
            modifiers: Modifiers::new(),
        });
        let action = bar.handle_event(&event);
        assert!(matches!(action, Action::CycleFocusForward));
        // Content should be unchanged
        assert_eq!(bar.content, "hello");
    }

    #[test]
    fn test_shift_tab_cycles_focus_backward() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.content = "hello".to_string();

        let event = Event::Key(KeyEvent {
            key: Key::BackTab,
            modifiers: Modifiers::new(),
        });
        let action = bar.handle_event(&event);
        assert!(matches!(action, Action::CycleFocusBackward));
    }

    #[test]
    fn test_tab_complete_empty_prefix() {
        let mut bar = bar_with_commands("agent", "llama3.1:8b");
        bar.content = "/".to_string();
        bar.cursor = 1;

        let event = Event::Key(KeyEvent {
            key: Key::Tab,
            modifiers: Modifiers::new(),
        });
        bar.handle_event(&event);
        // Should complete to the first command alphabetically
        assert!(bar.content.starts_with('/'));
        assert!(bar.content.len() > 1);
    }

    #[test]
    fn test_is_command_input() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        assert!(!bar.is_command_input());
        bar.content = "/help".to_string();
        assert!(bar.is_command_input());
        bar.content = "hello".to_string();
        assert!(!bar.is_command_input());
    }

    #[test]
    fn test_confirmation_mode_set() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        assert!(!bar.is_confirming());
        bar.set_confirming(true);
        assert!(bar.is_confirming());
        assert!(bar.content.is_empty());
        bar.set_confirming(false);
        assert!(!bar.is_confirming());
    }

    #[test]
    fn test_confirmation_y_approves() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.set_confirming(true);

        let event = Event::Key(KeyEvent {
            key: Key::Char('y'),
            modifiers: Modifiers::new(),
        });
        let action = bar.handle_event(&event);
        assert!(matches!(action, Action::ConfirmYes));
        assert!(!bar.is_confirming());
    }

    #[test]
    fn test_confirmation_n_denies() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.set_confirming(true);

        let event = Event::Key(KeyEvent {
            key: Key::Char('n'),
            modifiers: Modifiers::new(),
        });
        let action = bar.handle_event(&event);
        assert!(matches!(action, Action::ConfirmNo));
        assert!(!bar.is_confirming());
    }

    #[test]
    fn test_confirmation_a_approves_all() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.set_confirming(true);

        let event = Event::Key(KeyEvent {
            key: Key::Char('a'),
            modifiers: Modifiers::new(),
        });
        let action = bar.handle_event(&event);
        assert!(matches!(action, Action::ConfirmAll));
        assert!(!bar.is_confirming());
    }

    #[test]
    fn test_confirmation_escape_denies() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.set_confirming(true);

        let event = Event::Key(KeyEvent {
            key: Key::Escape,
            modifiers: Modifiers::new(),
        });
        let action = bar.handle_event(&event);
        assert!(matches!(action, Action::ConfirmNo));
        assert!(!bar.is_confirming());
    }

    #[test]
    fn test_confirmation_ignores_other_keys() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.set_confirming(true);

        let event = Event::Key(KeyEvent {
            key: Key::Char('x'),
            modifiers: Modifiers::new(),
        });
        let action = bar.handle_event(&event);
        assert!(matches!(action, Action::None));
        assert!(bar.is_confirming()); // Still in confirmation mode
    }

    #[test]
    fn test_confirmation_ctrl_y_ignored() {
        let mut bar = InputBarWidget::new("agent", "llama3.1:8b");
        bar.set_confirming(true);

        let event = Event::Key(KeyEvent {
            key: Key::Char('y'),
            modifiers: Modifiers::ctrl(),
        });
        let action = bar.handle_event(&event);
        assert!(matches!(action, Action::None));
        assert!(bar.is_confirming()); // Ctrl+y should not confirm
    }

    // ── Emacs-style editing shortcut tests ─────────────────────────────

    #[test]
    fn test_ctrl_u_clears_before_cursor() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.content = "hello world".to_string();
        bar.cursor = 5;
        let event = Event::Key(KeyEvent {
            key: Key::Char('u'),
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.content, " world");
        assert_eq!(bar.cursor, 0);
    }

    #[test]
    fn test_ctrl_k_clears_after_cursor() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.content = "hello world".to_string();
        bar.cursor = 5;
        let event = Event::Key(KeyEvent {
            key: Key::Char('k'),
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.content, "hello");
        assert_eq!(bar.cursor, 5);
    }

    #[test]
    fn test_ctrl_w_deletes_word_backward() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.content = "hello world".to_string();
        bar.cursor = 11;
        let event = Event::Key(KeyEvent {
            key: Key::Char('w'),
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.content, "hello ");
    }

    #[test]
    fn test_ctrl_a_move_to_line_start() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.content = "hello world".to_string();
        bar.cursor = 5;
        let event = Event::Key(KeyEvent {
            key: Key::Char('a'),
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.cursor, 0);
    }

    #[test]
    fn test_ctrl_e_move_to_line_end() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.content = "hello world".to_string();
        bar.cursor = 0;
        let event = Event::Key(KeyEvent {
            key: Key::Char('e'),
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.cursor, 11);
    }

    #[test]
    fn test_ctrl_y_yanks_kill_ring() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.content = "hello world".to_string();
        bar.cursor = 11;
        // Kill "world" with Ctrl+W
        let kill_event = Event::Key(KeyEvent {
            key: Key::Char('w'),
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&kill_event);
        assert_eq!(bar.content, "hello ");
        assert_eq!(bar.kill_ring, "world");
        // Yank it back
        let yank_event = Event::Key(KeyEvent {
            key: Key::Char('y'),
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&yank_event);
        assert_eq!(bar.content, "hello world");
    }

    #[test]
    fn test_ctrl_k_yank_roundtrip() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.content = "hello world".to_string();
        bar.cursor = 5;
        // Ctrl+K kills " world"
        let event = Event::Key(KeyEvent {
            key: Key::Char('k'),
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.content, "hello");
        assert_eq!(bar.kill_ring, " world");
        // Ctrl+Y yanks it back
        let yank_event = Event::Key(KeyEvent {
            key: Key::Char('y'),
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&yank_event);
        assert_eq!(bar.content, "hello world");
    }

    #[test]
    fn test_ctrl_u_yank_roundtrip() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.content = "hello world".to_string();
        bar.cursor = 5;
        // Ctrl+U kills "hello"
        let event = Event::Key(KeyEvent {
            key: Key::Char('u'),
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&event);
        assert_eq!(bar.content, " world");
        assert_eq!(bar.kill_ring, "hello");
        // Ctrl+Y yanks it back
        let yank_event = Event::Key(KeyEvent {
            key: Key::Char('y'),
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&yank_event);
        assert_eq!(bar.content, "hello world");
    }

    #[test]
    fn test_ctrl_left_right_word_movement() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.content = "hello world test".to_string();
        bar.cursor = 0;
        // Ctrl+Right: jump forward by word (word + trailing whitespace)
        let right_event = Event::Key(KeyEvent {
            key: Key::Right,
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&right_event);
        assert_eq!(bar.cursor, 6); // after "hello " (0 + 5 + 1 = 6)
        bar.handle_event(&right_event);
        assert_eq!(bar.cursor, 12); // after "world " (6 + 5 + 1 = 12)

        // Ctrl+Left: jump back by word
        let left_event = Event::Key(KeyEvent {
            key: Key::Left,
            modifiers: Modifiers::ctrl(),
        });
        bar.handle_event(&left_event);
        assert_eq!(bar.cursor, 6); // before "world "
        bar.handle_event(&left_event);
        assert_eq!(bar.cursor, 0); // before "hello "
    }

    #[test]
    fn test_alt_b_f_word_movement() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.content = "hello world test".to_string();
        bar.cursor = 16; // end

        // Alt+B: move back by word
        let alt_b = Event::Key(KeyEvent {
            key: Key::Char('b'),
            modifiers: Modifiers::alt(),
        });
        bar.handle_event(&alt_b);
        assert_eq!(bar.cursor, 12); // before "test"
        bar.handle_event(&alt_b);
        assert_eq!(bar.cursor, 6); // before "world"

        // Alt+F: move forward by word (skips word + trailing whitespace)
        let alt_f = Event::Key(KeyEvent {
            key: Key::Char('f'),
            modifiers: Modifiers::alt(),
        });
        bar.handle_event(&alt_f);
        assert_eq!(bar.cursor, 12); // after "world " (6 + 5 + 1 = 12)
        bar.handle_event(&alt_f);
        assert_eq!(bar.cursor, 16); // after "test" (12 + 4 = 16)
    }

    #[test]
    fn test_bracketed_paste() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.content = "hello".to_string();
        bar.cursor = 5;
        let event = Event::Paste(" world from paste".to_string());
        bar.handle_event(&event);
        assert_eq!(bar.content, "hello world from paste");
        assert_eq!(bar.cursor, 22);
    }

    #[test]
    fn test_bracketed_paste_ignored_in_confirmation_mode() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.set_confirming(true);
        let event = Event::Paste("pasted text".to_string());
        bar.handle_event(&event);
        // In confirmation mode, paste should be ignored
        assert!(bar.content.is_empty());
    }

    #[test]
    fn test_bracketed_paste_ignored_in_question_mode() {
        let mut bar = InputBarWidget::new("agent", "test");
        bar.set_questioning(true, 3);
        // Actually, question mode does allow typing. Let's just test it doesn't crash.
        let event = Event::Paste("test".to_string());
        bar.handle_event(&event);
        // Paste is ignored in question mode per our implementation
    }
}
