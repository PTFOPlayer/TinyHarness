// ── Status bar widget ───────────────────────────────────────────────────────
//
// Displays mode, model name, token count, session name, and message count
// at the top of the TUI screen.

use crate::tui::cell::{Cell, Color, Style};
use crate::tui::layout::Rect;
use crate::tui::screen::Screen;
use crate::tui::widget::{Widget, styles};

/// The status bar at the top of the screen.
///
/// Shows: mode label | model name | token count | message count | session name
///
/// Example (not valid Rust — for illustration only):
///
/// ```text
/// [agent] TinyHarness | llama3.1:8b | 12.4k/128k (9.6%) | 42 msgs | 3 files | session-name
/// ```
pub struct StatusBarWidget {
    pub mode_label: String,
    pub mode_color: Color,
    pub model_name: String,
    pub token_count: Option<(u32, u32)>, // (used, total)
    pub message_count: usize,
    pub pinned_file_count: usize,
    pub session_name: String,
    pub is_streaming: bool,
    /// Label for the currently focused widget (e.g., "input", "chat", "sidebar", "files").
    pub focus_label: String,
}

impl StatusBarWidget {
    pub fn new(mode_label: &str, model_name: &str) -> Self {
        let mode_color = match mode_label {
            "casual" => styles::MODE_CASUAL_FG,
            "planning" => styles::MODE_PLANNING_FG,
            "agent" => styles::MODE_AGENT_FG,
            "research" => styles::MODE_RESEARCH_FG,
            _ => Color::WHITE,
        };

        Self {
            mode_label: mode_label.to_string(),
            mode_color,
            model_name: model_name.to_string(),
            token_count: None,
            message_count: 0,
            pinned_file_count: 0,
            session_name: String::from("unnamed"),
            is_streaming: false,
            focus_label: String::from("input"),
        }
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

    /// Set the session name.
    pub fn set_session_name(&mut self, name: &str) {
        self.session_name = name.to_string();
    }

    /// Set the message count.
    pub fn set_message_count(&mut self, count: usize) {
        self.message_count = count;
    }

    /// Set the token count (used, total).
    pub fn set_token_count(&mut self, used: u64, total: Option<u64>) {
        self.token_count = total.map(|t| (used as u32, t as u32));
    }

    /// Set whether the assistant is currently streaming.
    pub fn set_streaming(&mut self, streaming: bool) {
        self.is_streaming = streaming;
    }

    /// Set the focus indicator label (e.g., "input", "chat", "files").
    pub fn set_focus_label(&mut self, label: &str) {
        self.focus_label = label.to_string();
    }

    /// Format token count for display.
    fn format_tokens(&self) -> String {
        match self.token_count {
            Some((used, total)) => {
                let used_str = if used >= 1000 {
                    format!("{:.1}k", used as f64 / 1000.0)
                } else {
                    used.to_string()
                };
                let total_str = if total >= 1000 {
                    format!("{:.0}k", total as f64 / 1000.0)
                } else {
                    total.to_string()
                };
                let pct = if total > 0 {
                    format!("{:.0}%", (used as f64 / total as f64) * 100.0)
                } else {
                    "?%".to_string()
                };
                format!("{used_str}/{total_str} ({pct})")
            }
            None => "? tokens".to_string(),
        }
    }
}

impl Widget for StatusBarWidget {
    fn render(&mut self, area: Rect, screen: &mut Screen) {
        if area.is_empty() || area.height < 1 {
            return;
        }

        // Fill the status bar background
        screen.fill_rect(
            area,
            Cell {
                char: ' ',
                fg: styles::STATUS_BAR_FG,
                bg: styles::STATUS_BAR_BG,
                style: Style::default(),
            },
        );

        // Build the status line content
        let row = area.y;

        // Mode label with color
        let mode_text = format!(" {} ", self.mode_label);
        screen.write_str(
            row,
            area.x,
            &mode_text,
            self.mode_color,
            styles::STATUS_BAR_BG,
            Style::bold(),
        );

        // Separator
        let mut col = area.x + mode_text.len() as u16;
        screen.write_str(
            row,
            col,
            " │ ",
            Color::Ansi(240),
            styles::STATUS_BAR_BG,
            Style::default(),
        );
        col += 3;

        // Model name
        let model_text = &self.model_name;
        screen.write_str(
            row,
            col,
            model_text,
            Color::WHITE,
            styles::STATUS_BAR_BG,
            Style::default(),
        );
        col += model_text.len() as u16;

        // Separator
        screen.write_str(
            row,
            col,
            " │ ",
            Color::Ansi(240),
            styles::STATUS_BAR_BG,
            Style::default(),
        );
        col += 3;

        // Token count
        let token_text = self.format_tokens();
        let token_color = match self.token_count {
            Some((used, total)) if total > 0 => {
                let pct = used as f64 / total as f64;
                if pct > 0.9 {
                    Color::RED
                } else if pct > 0.7 {
                    Color::YELLOW
                } else {
                    Color::GREEN
                }
            }
            _ => Color::GRAY,
        };
        screen.write_str(
            row,
            col,
            &token_text,
            token_color,
            styles::STATUS_BAR_BG,
            Style::default(),
        );
        col += token_text.len() as u16;

        // Separator
        screen.write_str(
            row,
            col,
            " │ ",
            Color::Ansi(240),
            styles::STATUS_BAR_BG,
            Style::default(),
        );
        col += 3;

        // Message count
        let msg_text = format!("{} msgs", self.message_count);
        screen.write_str(
            row,
            col,
            &msg_text,
            Color::WHITE,
            styles::STATUS_BAR_BG,
            Style::dim(),
        );
        col += msg_text.len() as u16;

        // Pinned files
        if self.pinned_file_count > 0 {
            screen.write_str(
                row,
                col,
                " │ ",
                Color::Ansi(240),
                styles::STATUS_BAR_BG,
                Style::default(),
            );
            col += 3;
            let file_text = format!("{} files", self.pinned_file_count);
            screen.write_str(
                row,
                col,
                &file_text,
                Color::WHITE,
                styles::STATUS_BAR_BG,
                Style::dim(),
            );
            col += file_text.len() as u16;
        }

        // Focus indicator (right of left-section, before session name)
        let focus_text = format!(" ▸ {} ", self.focus_label);
        let focus_color = Color::Ansi(178); // warm amber to stand out
        // Place it just before the session name if there's room
        let session_text = format!(" {} ", self.session_name);
        let session_start = area.x + area.width.saturating_sub(session_text.len() as u16);
        let focus_start = session_start.saturating_sub(focus_text.len() as u16);
        if focus_start > col {
            screen.write_str(
                row,
                focus_start,
                &focus_text,
                focus_color,
                styles::STATUS_BAR_BG,
                Style::bold(),
            );
        }

        // Session name (right-aligned)
        if session_start > col {
            screen.write_str(
                row,
                session_start,
                &session_text,
                Color::Ansi(240),
                styles::STATUS_BAR_BG,
                Style::default(),
            );
        }

        // Streaming indicator
        if self.is_streaming && col + 4 < area.x + area.width {
            screen.write_str(
                row,
                col,
                " ⋯",
                Color::ORANGE,
                styles::STATUS_BAR_BG,
                Style::default(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_bar_render() {
        let mut screen = Screen::new(80, 24);
        let mut widget = StatusBarWidget::new("agent", "llama3.1:8b");
        let area = Rect::new(0, 0, 80, 1);
        widget.render(area, &mut screen);

        // Should have content in the first row
        assert_ne!(screen.get(0, 0).unwrap().char, '\0');
    }

    #[test]
    fn test_format_tokens() {
        let widget = StatusBarWidget::new("agent", "llama3.1:8b");
        assert_eq!(widget.format_tokens(), "? tokens");

        let mut widget = StatusBarWidget::new("agent", "llama3.1:8b");
        widget.token_count = Some((12000, 128000));
        let tokens = widget.format_tokens();
        // 12000/128000 = 9.375%, which rounds to 9%
        assert!(tokens.contains("%") && tokens.contains("12.0k") && tokens.contains("128k"));
    }

    #[test]
    fn test_status_bar_with_tokens() {
        let mut screen = Screen::new(80, 24);
        let mut widget = StatusBarWidget::new("agent", "llama3.1:8b");
        widget.token_count = Some((5000, 128000));
        widget.message_count = 42;
        let area = Rect::new(0, 0, 80, 1);
        widget.render(area, &mut screen);

        // Should have rendered content
        assert_ne!(screen.get(0, 0).unwrap().char, '\0');
    }
}
