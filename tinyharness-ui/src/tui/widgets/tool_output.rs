// ── Tool output widget ────────────────────────────────────────────────────────
//
// Displays tool call results in a collapsible pane. Tool results start
// collapsed (just header line). Click or Enter to expand.

use unicode_width::UnicodeWidthStr;

use crate::tui::cell::{Cell, Color, Style};
use crate::tui::event::{Event, Key, KeyEvent, Modifiers, MouseEvent};
use crate::tui::layout::Rect;
use crate::tui::screen::Screen;
use crate::tui::widget::{Action, Widget, truncate_str_width};

/// Status of a tool call.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolStatus {
    /// The tool is currently running.
    Running,
    /// The tool completed successfully (with duration in ms).
    Success { duration_ms: u64 },
    /// The tool failed with an error message.
    Error { message: String },
}

/// A single tool result entry.
#[derive(Clone, Debug)]
pub struct ToolResult {
    /// Tool name (e.g., "read", "run", "write").
    pub name: String,
    /// Brief summary of arguments (e.g., "src/main.rs:42-58").
    pub args_summary: String,
    /// The output content.
    pub content: String,
    /// Whether the result is an error.
    pub is_error: bool,
    /// Whether the result is collapsed.
    pub collapsed: bool,
    /// Tool execution status.
    pub status: ToolStatus,
}

/// Collapsible tool result display.
///
/// Shows a list of tool results, each of which can be expanded
/// or collapsed. This is used inside the conversation widget or
/// as a standalone pane.
pub struct ToolOutputWidget {
    /// Tool results to display.
    results: Vec<ToolResult>,
    /// Currently selected/expanded result index.
    selected: Option<usize>,
    /// Scroll offset for the content area.
    scroll_offset: usize,
}

impl ToolOutputWidget {
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
            selected: None,
            scroll_offset: 0,
        }
    }

    /// Add a tool result.
    pub fn push(&mut self, result: ToolResult) {
        self.results.push(result);
    }

    /// Clear all results.
    pub fn clear(&mut self) {
        self.results.clear();
        self.selected = None;
        self.scroll_offset = 0;
    }

    /// Get the number of results.
    pub fn len(&self) -> usize {
        self.results.len()
    }

    /// Check if there are no results.
    pub fn is_empty(&self) -> bool {
        self.results.is_empty()
    }

    /// Toggle collapse/expand for the given result index.
    pub fn toggle(&mut self, index: usize) {
        if index < self.results.len() {
            self.results[index].collapsed = !self.results[index].collapsed;
            if !self.results[index].collapsed {
                self.selected = Some(index);
                self.scroll_offset = 0;
            }
        }
    }

    /// Uncollapse all results (expand everything for viewing).
    pub fn un_collapse_all(&mut self) {
        for result in &mut self.results {
            result.collapsed = false;
        }
    }

    /// Render a collapsed result (single line header).
    fn render_collapsed(&self, result: &ToolResult, row: u16, screen: &mut Screen, width: u16) {
        let status_icon = match &result.status {
            ToolStatus::Running => "⟳",
            ToolStatus::Success { .. } => "✓",
            ToolStatus::Error { .. } => "✗",
        };

        let status_color = match &result.status {
            ToolStatus::Running => Color::YELLOW,
            ToolStatus::Success { .. } => Color::GREEN,
            ToolStatus::Error { .. } => Color::RED,
        };

        let duration_str = match &result.status {
            ToolStatus::Success { duration_ms } => format!(" {}ms", duration_ms),
            _ => String::new(),
        };

        // Format: "  ✓ Tool: read src/main.rs:42-58  120ms"
        let header = if result.args_summary.is_empty() {
            format!("  {} Tool: {}", status_icon, result.name)
        } else {
            format!(
                "  {} Tool: {} {}",
                status_icon, result.name, result.args_summary
            )
        };

        let header_with_dur = format!("{}{}", header, duration_str);
        let header_width = header_with_dur.width();

        screen.write_str(
            row,
            0,
            &header_with_dur,
            status_color,
            Color::Default,
            Style::bold(),
        );

        // Show content preview (first line, truncated)
        if !result.content.is_empty() {
            let preview_col = (header_width as u16 + 2).min(width.saturating_sub(20));
            let available = (width as usize).saturating_sub(preview_col as usize);
            if available > 10 {
                let first_line = result.content.lines().next().unwrap_or("");
                let preview = if first_line.width() > available.saturating_sub(3) {
                    format!(
                        "{}…",
                        truncate_str_width(first_line, available.saturating_sub(3))
                    )
                } else {
                    first_line.to_string()
                };
                screen.write_str(
                    row,
                    preview_col,
                    &preview,
                    Color::Ansi(244),
                    Color::Default,
                    Style::dim(),
                );
            }
        }

        // Show expand hint at the far right
        let hint_col = width.saturating_sub(3);
        screen.write_str(
            row,
            hint_col,
            " ▶",
            Color::Ansi(240),
            Color::Default,
            Style::dim(),
        );
    }

    /// Render an expanded result (header + content).
    fn render_expanded(
        &self,
        result: &ToolResult,
        row: u16,
        screen: &mut Screen,
        width: u16,
        max_rows: u16,
    ) -> u16 {
        let status_icon = match &result.status {
            ToolStatus::Running => "⟳",
            ToolStatus::Success { .. } => "✓",
            ToolStatus::Error { .. } => "✗",
        };

        let status_color = match &result.status {
            ToolStatus::Running => Color::YELLOW,
            ToolStatus::Success { .. } => Color::GREEN,
            ToolStatus::Error { .. } => Color::RED,
        };

        // Header line
        let header = if result.args_summary.is_empty() {
            format!("  {} Tool: {}", status_icon, result.name)
        } else {
            format!(
                "  {} Tool: {} {}",
                status_icon, result.name, result.args_summary
            )
        };
        screen.write_str(row, 0, &header, status_color, Color::Default, Style::bold());

        // Collapse hint at far right
        let hint_col = width.saturating_sub(3);
        if row < row + max_rows {
            screen.write_str(
                row,
                hint_col,
                " ▼",
                Color::Ansi(240),
                Color::Default,
                Style::dim(),
            );
        }

        // Content lines
        let content_color = if result.is_error {
            Color::RED
        } else {
            Color::Ansi(252)
        };
        let content_bg = if result.is_error {
            Color::Ansi(52) // dark red bg
        } else {
            Color::Default
        };

        let lines: Vec<&str> = result.content.lines().collect();
        let mut current_row = row + 1;

        for (i, line) in lines.iter().enumerate() {
            if i < self.scroll_offset {
                continue;
            }
            if current_row >= row + max_rows {
                break;
            }

            // Draw content with background and left border
            screen.write_str(
                current_row,
                0,
                "  │",
                Color::Ansi(240),
                content_bg,
                Style::default(),
            );

            let available = (width as usize).saturating_sub(4);
            let display = if line.width() > available {
                format!("{}…", truncate_str_width(line, available.saturating_sub(1)))
            } else {
                line.to_string()
            };
            screen.write_str(
                current_row,
                3,
                &display,
                content_color,
                content_bg,
                Style::default(),
            );

            // Fill the rest of the line with background (only for errors)
            if result.is_error {
                let end_col = 3 + display.len() as u16;
                if end_col < width {
                    for c in end_col..width {
                        if let Some(cell) = screen.get_mut(current_row, c) {
                            cell.bg = content_bg;
                        }
                    }
                }
            }

            current_row += 1;
        }

        // Bottom border of expanded section
        if current_row < row + max_rows {
            screen.write_str(
                current_row,
                0,
                "  └",
                Color::Ansi(240),
                Color::Default,
                Style::default(),
            );
            screen.hline(
                current_row,
                3,
                width.saturating_sub(4),
                '─',
                Color::Ansi(240),
                Color::Default,
            );
            current_row += 1;
        }

        // Lines used
        current_row - row
    }
}

impl Widget for ToolOutputWidget {
    fn render(&mut self, area: Rect, screen: &mut Screen) {
        if area.is_empty() || self.results.is_empty() {
            return;
        }

        // Clear the area
        screen.fill_rect(area, Cell::default());

        let mut row = area.y;
        let width = area.width;

        for (i, result) in self.results.iter().enumerate() {
            if row >= area.y + area.height {
                break;
            }

            if result.collapsed {
                self.render_collapsed(result, row, screen, width);
                row += 1;
            } else {
                let remaining = area.y + area.height - row;
                let lines_used = self.render_expanded(result, row, screen, width, remaining);
                row += lines_used;
            }

            // Add a small gap between results (if space allows)
            if i < self.results.len() - 1 && row < area.y + area.height {
                // Just leave a blank line
                row += 1;
            }
        }
    }

    fn handle_event(&mut self, event: &Event) -> Action {
        match event {
            Event::Key(KeyEvent {
                key: Key::Enter,
                modifiers:
                    Modifiers {
                        ctrl: false,
                        alt: false,
                        shift: false,
                    },
            }) => {
                // Toggle the selected result
                if let Some(idx) = self.selected {
                    self.toggle(idx);
                } else if !self.results.is_empty() {
                    self.toggle(0);
                }
                Action::None
            }
            Event::Key(KeyEvent {
                key: Key::Up,
                modifiers: Modifiers { alt: true, .. },
            }) => {
                // Navigate up in the result list
                if let Some(idx) = self.selected {
                    if idx > 0 {
                        self.selected = Some(idx - 1);
                    }
                } else if !self.results.is_empty() {
                    self.selected = Some(self.results.len() - 1);
                }
                Action::None
            }
            Event::Key(KeyEvent {
                key: Key::Down,
                modifiers: Modifiers { alt: true, .. },
            }) => {
                // Navigate down in the result list
                if let Some(idx) = self.selected {
                    if idx + 1 < self.results.len() {
                        self.selected = Some(idx + 1);
                    }
                } else if !self.results.is_empty() {
                    self.selected = Some(0);
                }
                Action::None
            }
            Event::Mouse(MouseEvent::Press { row, .. }) => {
                // Click to toggle the result at this row
                // Simple heuristic: each collapsed result takes 1 row,
                // expanded results take variable rows
                let mut current_row = 0u16;
                for (i, result) in self.results.iter().enumerate() {
                    if *row == current_row {
                        self.toggle(i);
                        self.selected = Some(i);
                        break;
                    }
                    if result.collapsed {
                        current_row += 1;
                    } else {
                        // Approximate: count content lines + header + border
                        let line_count = result.content.lines().count() as u16;
                        current_row += line_count + 2; // header + bottom border
                    }
                    current_row += 1; // gap
                }
                Action::None
            }
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_output_new() {
        let widget = ToolOutputWidget::new();
        assert!(widget.is_empty());
        assert_eq!(widget.len(), 0);
    }

    #[test]
    fn test_tool_output_push() {
        let mut widget = ToolOutputWidget::new();
        widget.push(ToolResult {
            name: "read".to_string(),
            args_summary: "src/main.rs".to_string(),
            content: "fn main() {}".to_string(),
            is_error: false,
            collapsed: true,
            status: ToolStatus::Success { duration_ms: 42 },
        });
        assert_eq!(widget.len(), 1);
    }

    #[test]
    fn test_tool_output_toggle() {
        let mut widget = ToolOutputWidget::new();
        widget.push(ToolResult {
            name: "read".to_string(),
            args_summary: "src/main.rs".to_string(),
            content: "fn main() {}".to_string(),
            is_error: false,
            collapsed: true,
            status: ToolStatus::Success { duration_ms: 42 },
        });
        assert!(widget.results[0].collapsed);

        widget.toggle(0);
        assert!(!widget.results[0].collapsed);

        widget.toggle(0);
        assert!(widget.results[0].collapsed);
    }

    #[test]
    fn test_tool_output_clear() {
        let mut widget = ToolOutputWidget::new();
        widget.push(ToolResult {
            name: "read".to_string(),
            args_summary: String::new(),
            content: String::new(),
            is_error: false,
            collapsed: true,
            status: ToolStatus::Success { duration_ms: 0 },
        });
        widget.clear();
        assert!(widget.is_empty());
    }

    #[test]
    fn test_tool_output_render_collapsed() {
        let mut screen = Screen::new(80, 24);
        let widget = ToolOutputWidget::new();
        let mut widget = widget;
        widget.push(ToolResult {
            name: "read".to_string(),
            args_summary: "src/main.rs".to_string(),
            content: "fn main() {}".to_string(),
            is_error: false,
            collapsed: true,
            status: ToolStatus::Success { duration_ms: 42 },
        });

        let area = Rect::new(0, 0, 80, 24);
        widget.render(area, &mut screen);

        // First cell should have content (the status icon)
        assert!(screen.get(0, 0).unwrap().char != '\0');
    }

    #[test]
    fn test_tool_output_render_expanded() {
        let mut screen = Screen::new(80, 24);
        let mut widget = ToolOutputWidget::new();
        widget.push(ToolResult {
            name: "read".to_string(),
            args_summary: "src/main.rs".to_string(),
            content: "fn main() {}\nfn other() {}".to_string(),
            is_error: false,
            collapsed: false,
            status: ToolStatus::Success { duration_ms: 42 },
        });

        let area = Rect::new(0, 0, 80, 24);
        widget.render(area, &mut screen);

        // Should have rendered header and content lines
        assert!(screen.get(0, 0).unwrap().char != '\0');
    }

    #[test]
    fn test_tool_output_render_error() {
        let mut screen = Screen::new(80, 24);
        let mut widget = ToolOutputWidget::new();
        widget.push(ToolResult {
            name: "run".to_string(),
            args_summary: "cargo test".to_string(),
            content: "error: test failed".to_string(),
            is_error: true,
            collapsed: true,
            status: ToolStatus::Error {
                message: "test failed".to_string(),
            },
        });

        let area = Rect::new(0, 0, 80, 24);
        widget.render(area, &mut screen);

        // Should render with error styling
        assert!(screen.get(0, 0).unwrap().char != '\0');
    }

    #[test]
    fn test_tool_output_keyboard_toggle() {
        let mut widget = ToolOutputWidget::new();
        widget.push(ToolResult {
            name: "read".to_string(),
            args_summary: String::new(),
            content: "hello".to_string(),
            is_error: false,
            collapsed: true,
            status: ToolStatus::Success { duration_ms: 0 },
        });

        // Press Enter to expand
        let event = Event::Key(KeyEvent {
            key: Key::Enter,
            modifiers: Modifiers::new(),
        });
        widget.handle_event(&event);
        assert!(!widget.results[0].collapsed);

        // Press Enter again to collapse
        widget.handle_event(&event);
        assert!(widget.results[0].collapsed);
    }

    #[test]
    fn test_tool_status_equality() {
        let s1 = ToolStatus::Success { duration_ms: 100 };
        let s2 = ToolStatus::Success { duration_ms: 100 };
        assert_eq!(s1, s2);

        let e1 = ToolStatus::Error {
            message: "fail".to_string(),
        };
        let e2 = ToolStatus::Error {
            message: "fail".to_string(),
        };
        assert_eq!(e1, e2);

        assert_ne!(s1, ToolStatus::Running);
    }
}
