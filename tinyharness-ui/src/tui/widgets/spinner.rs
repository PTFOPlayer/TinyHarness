// ── Spinner widget ──────────────────────────────────────────────────────────
//
// Animated spinner for streaming responses.

use crate::tui::cell::{Color, Style};
use crate::tui::layout::Rect;
use crate::tui::screen::Screen;
use crate::tui::widget::Widget;

use crate::style::SPINNER_FRAMES;

/// A simple animated spinner shown during streaming responses.
pub struct SpinnerWidget {
    /// Current frame index.
    frame: usize,
    /// Label text (e.g., "Thinking", "Processing").
    label: String,
    /// Whether the spinner is active (visible).
    active: bool,
}

impl SpinnerWidget {
    pub fn new(label: &str) -> Self {
        Self {
            frame: 0,
            label: label.to_string(),
            active: false,
        }
    }

    /// Advance the spinner to the next frame.
    pub fn tick(&mut self) {
        if self.active {
            self.frame = (self.frame + 1) % SPINNER_FRAMES.len();
        }
    }

    /// Start the spinner.
    pub fn start(&mut self) {
        self.active = true;
        self.frame = 0;
    }

    /// Stop the spinner.
    pub fn stop(&mut self) {
        self.active = false;
    }

    /// Update the label text.
    pub fn set_label(&mut self, label: &str) {
        self.label = label.to_string();
    }
}

impl Widget for SpinnerWidget {
    fn render(&mut self, area: Rect, screen: &mut Screen) {
        if !self.active || area.is_empty() {
            return;
        }

        let row = area.y;
        let col = area.x;
        let max_col = area.x + area.width; // exclusive bound — clip here

        if let Some(frame) = SPINNER_FRAMES.get(self.frame) {
            // Draw spinner character
            if col < max_col {
                screen.write_str(
                    row,
                    col,
                    frame,
                    Color::ORANGE,
                    Color::Default,
                    Style::default(),
                );
            }

            // Draw label (clipped to area width)
            let label_col = col + 2;
            if label_col < max_col {
                let available = (max_col - label_col) as usize;
                let label = format!("{}…", self.label);
                let clipped = if label.len() > available {
                    // Truncate to fit within the area, preserving char boundaries
                    let mut end = available;
                    while end > 0 && !label.is_char_boundary(end) {
                        end -= 1;
                    }
                    &label[..end]
                } else {
                    label.as_str()
                };
                screen.write_str(
                    row,
                    label_col,
                    clipped,
                    Color::Ansi(8),
                    Color::Default,
                    Style::dim(),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spinner_new() {
        let spinner = SpinnerWidget::new("Thinking");
        assert!(!spinner.active);
        assert_eq!(spinner.label, "Thinking");
    }

    #[test]
    fn test_spinner_start_stop() {
        let mut spinner = SpinnerWidget::new("Thinking");
        assert!(!spinner.active);

        spinner.start();
        assert!(spinner.active);

        spinner.stop();
        assert!(!spinner.active);
    }

    #[test]
    fn test_spinner_tick() {
        let mut spinner = SpinnerWidget::new("Thinking");
        spinner.start();
        assert_eq!(spinner.frame, 0);

        spinner.tick();
        assert_eq!(spinner.frame, 1);

        // Wrap around
        for _ in 0..9 {
            spinner.tick();
        }
        assert_eq!(spinner.frame, 0); // Wrapped to start
    }

    #[test]
    fn test_spinner_tick_not_active() {
        let mut spinner = SpinnerWidget::new("Thinking");
        // Not active — tick should not advance
        spinner.tick();
        assert_eq!(spinner.frame, 0);
    }

    #[test]
    fn test_spinner_render() {
        let mut screen = Screen::new(80, 24);
        let mut spinner = SpinnerWidget::new("Thinking");
        spinner.start();

        let area = Rect::new(0, 0, 20, 1);
        spinner.render(area, &mut screen);

        // Should have rendered the spinner character
        assert_ne!(screen.get(0, 0).unwrap().char, ' ');
    }

    #[test]
    fn test_spinner_render_not_active() {
        let mut screen = Screen::new(80, 24);
        let mut spinner = SpinnerWidget::new("Thinking");
        // Not active — should render nothing

        let area = Rect::new(0, 0, 20, 1);
        spinner.render(area, &mut screen);

        // Should not have rendered anything
        assert_eq!(screen.get(0, 0).unwrap().char, ' ');
    }
}
