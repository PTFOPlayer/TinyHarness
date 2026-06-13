// ── Widget trait and action types ─────────────────────────────────────────────
//
// All TUI widgets implement the `Widget` trait, which provides a `render`
// method (draw to a screen buffer) and an `handle_event` method (process
// input events and optionally return an action).

use super::cell::Color;
use super::event::Event;
use super::layout::Rect;
use super::screen::Screen;

// ── Action type ─────────────────────────────────────────────────────────────

/// Actions that a widget can request from the application.
///
/// When a widget handles an event, it can optionally return an action
/// that the application should perform (e.g., sending a message,
/// switching modes, or quitting).
#[derive(Clone, Debug)]
pub enum Action {
    /// Send a message to the AI (user pressed Enter in the input bar).
    SendMessage(String),
    /// Switch to a different agent mode.
    SwitchMode(String),
    /// Scroll the conversation up.
    ScrollUp,
    /// Scroll the conversation down.
    ScrollDown,
    /// Scroll the conversation up by a page.
    PageUp,
    /// Scroll the conversation down by a page.
    PageDown,
    /// Toggle sidebar visibility.
    ToggleSidebar,
    /// Cycle focus forward (Tab without command input).
    CycleFocusForward,
    /// Cycle focus backward (Shift+Tab).
    CycleFocusBackward,
    /// Quit the application.
    Quit,
    /// User approved a tool confirmation (pressed 'y').
    ConfirmYes,
    /// User denied a tool confirmation (pressed 'n').
    ConfirmNo,
    /// User approved all future tool confirmations (pressed 'a').
    ConfirmAll,
    /// Exit structure/file browser mode (Escape at root directory).
    ExitStructureMode,
    /// User answered a question with their input text.
    AnswerQuestion(String),
    /// User requested to interrupt the current generation (Ctrl+C while streaming).
    Interrupt,
    /// No action — the event was handled internally.
    None,
}

// ── Widget trait ─────────────────────────────────────────────────────────────

/// A UI widget that can render itself to a screen buffer and handle events.
///
/// Widgets are the building blocks of the TUI. Each widget owns its own
/// state and can render itself into a rectangular area of the screen.
pub trait Widget {
    /// Render the widget into the given area of the screen buffer.
    ///
    /// This method should not write to the terminal directly. Instead, it
    /// writes cells to the screen buffer, which is then diff-rendered.
    fn render(&mut self, area: Rect, screen: &mut Screen);

    /// Handle an event and optionally return an action.
    ///
    /// Only the focused widget receives events. Other widgets should
    /// return `Action::None`.
    fn handle_event(&mut self, event: &Event) -> Action {
        let _ = event;
        Action::None
    }

    /// Whether this widget currently has keyboard focus.
    fn focused(&self) -> bool {
        false
    }

    /// Set whether this widget has keyboard focus.
    fn set_focus(&mut self, _focused: bool) {}
}

// ── Helper functions for widgets ─────────────────────────────────────────────

/// Style presets commonly used across widgets.
pub mod styles {
    use super::Color;

    /// Status bar background and text colors.
    pub const STATUS_BAR_FG: Color = Color::WHITE;
    pub const STATUS_BAR_BG: Color = Color::Ansi(236);

    /// Input bar background and text colors.
    pub const INPUT_BAR_FG: Color = Color::WHITE;
    pub const INPUT_BAR_BG: Color = Color::Ansi(235);

    /// Sidebar background and text colors.
    pub const SIDEBAR_FG: Color = Color::Ansi(252); // light gray
    pub const SIDEBAR_BG: Color = Color::Ansi(234); // dark gray
    pub const SIDEBAR_BORDER: Color = Color::Ansi(240); // medium gray

    /// Conversation text colors.
    pub const USER_MSG_FG: Color = Color::GREEN;
    pub const ASSISTANT_MSG_FG: Color = Color::WHITE;
    pub const TOOL_MSG_FG: Color = Color::Ansi(14); // bright cyan
    pub const THINKING_FG: Color = Color::Ansi(97); // dimmer magenta

    /// Scrollbar colors.
    pub const SCROLLBAR_FG: Color = Color::Ansi(244); // gray
    pub const SCROLLBAR_BG: Color = Color::Default;

    /// Mode label colors (matching existing CLI).
    pub const MODE_CASUAL_FG: Color = Color::GREEN;
    pub const MODE_PLANNING_FG: Color = Color::YELLOW;
    pub const MODE_AGENT_FG: Color = Color::CYAN;
    pub const MODE_RESEARCH_FG: Color = Color::ORANGE;

    /// Box drawing characters.
    pub const BOX_HORIZONTAL: char = '─';
    pub const BOX_VERTICAL: char = '│';
    pub const BOX_TOP_LEFT: char = '┌';
    pub const BOX_TOP_RIGHT: char = '┐';
    pub const BOX_BOTTOM_LEFT: char = '└';
    pub const BOX_BOTTOM_RIGHT: char = '┘';
    pub const BOX_LEFT_TEE: char = '├';
    pub const BOX_RIGHT_TEE: char = '┤';
    pub const BOX_TOP_TEE: char = '┬';
    pub const BOX_BOTTOM_TEE: char = '┴';
    pub const BOX_CROSS: char = '┼';
}

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Safely truncate a string to at most `max_len` bytes, respecting UTF-8
/// char boundaries. Returns a string slice that fits within `max_len` bytes.
///
/// Use this instead of `&s[..n]` which can panic if `n` lands inside a
/// multi-byte UTF-8 character (common with emoji, CJK, accented letters).
pub fn truncate_str(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    // Find the largest char boundary <= max_len
    let mut boundary = max_len;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &s[..boundary]
}

/// Truncate a string so that its Unicode display width is at most `max_width`.
///
/// Unlike `truncate_str`, this respects terminal display columns, so CJK and
/// emoji characters count correctly. Returns the longest prefix whose width
/// does not exceed `max_width`.
pub fn truncate_str_width(s: &str, max_width: usize) -> &str {
    if s.width() <= max_width {
        return s;
    }
    let mut acc = 0usize;
    for (idx, ch) in s.char_indices() {
        let w = ch.width().unwrap_or(1).max(1);
        if acc + w > max_width {
            return &s[..idx];
        }
        acc += w;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_str_ascii() {
        assert_eq!(truncate_str("hello", 3), "hel");
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello", 0), "");
    }

    #[test]
    fn test_truncate_str_multibyte() {
        // 🦀 is 4 bytes
        assert_eq!(truncate_str("🦀hello", 5), "🦀h"); // 🦀=4 bytes, +h=5
        assert_eq!(truncate_str("🦀hello", 3), ""); // 🦀=4 bytes, boundary=0
        // ⚙ is 3 bytes — truncating at byte 4 should include it
        let s = "⚙hello";
        assert_eq!(truncate_str(s, 4), "⚙h"); // ⚙=3 bytes + h=1 = 4
    }

    #[test]
    fn test_truncate_str_no_panic() {
        // Should never panic even with tricky byte boundaries
        let s = "naïve café 🦀";
        for i in 0..=s.len() + 5 {
            let _ = truncate_str(s, i);
        }
    }
}
