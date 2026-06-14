// ── Color representation for the TUI screen buffer ──────────────────────────
//
// Supports 4-bit (16 colors), 8-bit (256 colors), and 24-bit (true color)
// using raw ANSI escape sequences — no external TUI framework needed.

/// Terminal color representation.
///
/// Supports the full range of terminal colors:
/// - `Default`: use the terminal's default foreground/background
/// - `Ansi(n)`: 4-bit (0–15) or 8-bit (16–255) indexed color
/// - `Rgb(r, g, b)`: 24-bit true color
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    /// Terminal default color (inherits from terminal theme).
    Default,
    /// 4-bit (0–15) or 8-bit (16–255) indexed color.
    Ansi(u8),
    /// 24-bit true color.
    Rgb(u8, u8, u8),
}

// ── Named color constants (matching style.rs ANSI codes) ───────────────────

impl Color {
    // Standard foreground colors (4-bit)
    pub const BLACK: Color = Color::Ansi(0);
    pub const RED: Color = Color::Ansi(1);
    pub const GREEN: Color = Color::Ansi(2);
    pub const YELLOW: Color = Color::Ansi(3);
    pub const BLUE: Color = Color::Ansi(4);
    pub const MAGENTA: Color = Color::Ansi(5);
    pub const CYAN: Color = Color::Ansi(6);
    pub const WHITE: Color = Color::Ansi(7);

    // Bright / extended colors (matching style.rs)
    pub const GRAY: Color = Color::Ansi(8); // bright black
    pub const BRIGHT_RED: Color = Color::Ansi(9);
    pub const BRIGHT_GREEN: Color = Color::Ansi(10);
    pub const BRIGHT_YELLOW: Color = Color::Ansi(11);
    pub const BRIGHT_BLUE: Color = Color::Ansi(12);
    pub const BRIGHT_MAGENTA: Color = Color::Ansi(13);
    pub const BRIGHT_CYAN: Color = Color::Ansi(14);
    pub const BRIGHT_WHITE: Color = Color::Ansi(15);
    pub const ORANGE: Color = Color::Ansi(208);

    // Background colors (matching style.rs BG_* constants)
    pub const BG_DIM: Color = Color::Ansi(236);
    pub const BG_TOOL: Color = Color::Ansi(237);
    pub const BG_WARN: Color = Color::Ansi(17);
}

impl Color {
    /// Generate the ANSI escape sequence to set this color as the foreground.
    pub fn fg_escape(&self) -> String {
        match self {
            Color::Default => "\x1b[39m".to_string(),
            Color::Ansi(n) => {
                if *n < 8 {
                    // Standard colors: ESC[30–37m
                    format!("\x1b[{}m", 30 + *n as u16)
                } else if *n < 16 {
                    // Bright colors: ESC[90–97m
                    format!("\x1b[{}m", 90 + (*n - 8) as u16)
                } else {
                    // 8-bit colors: ESC[38;5;nm
                    format!("\x1b[38;5;{}m", n)
                }
            }
            Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"),
        }
    }

    /// Generate the ANSI escape sequence to set this color as the background.
    pub fn bg_escape(&self) -> String {
        match self {
            Color::Default => "\x1b[49m".to_string(),
            Color::Ansi(n) => {
                if *n < 8 {
                    // Standard background: ESC[40–47m
                    format!("\x1b[{}m", 40 + *n as u16)
                } else if *n < 16 {
                    // Bright background: ESC[100–107m
                    format!("\x1b[{}m", 100 + (*n - 8) as u16)
                } else {
                    // 8-bit background: ESC[48;5;nm
                    format!("\x1b[48;5;{}m", n)
                }
            }
            Color::Rgb(r, g, b) => format!("\x1b[48;2;{r};{g};{b}m"),
        }
    }
}

// ── Cell style flags ────────────────────────────────────────────────────────

/// Text style attributes that can be combined.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
}

impl Style {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn bold() -> Self {
        Self {
            bold: true,
            ..Self::default()
        }
    }

    pub fn dim() -> Self {
        Self {
            dim: true,
            ..Self::default()
        }
    }

    pub fn bold_dim() -> Self {
        Self {
            bold: true,
            dim: true,
            ..Self::default()
        }
    }

    pub fn blink() -> Self {
        Self {
            blink: true,
            ..Self::default()
        }
    }

    /// Generate the ANSI escape sequences for this style.
    pub fn escape(&self) -> String {
        let mut parts = Vec::new();
        if self.bold {
            parts.push("\x1b[1m");
        }
        if self.dim {
            parts.push("\x1b[2m");
        }
        if self.italic {
            parts.push("\x1b[3m");
        }
        if self.underline {
            parts.push("\x1b[4m");
        }
        if self.blink {
            parts.push("\x1b[5m");
        }
        parts.join("")
    }

    /// Generate the ANSI reset sequence to clear all styles.
    pub fn reset() -> &'static str {
        "\x1b[0m"
    }
}

// ── Screen cell ─────────────────────────────────────────────────────────────

/// A single cell in the screen buffer.
///
/// Each cell stores a character, foreground color, background color,
/// and style flags. When the screen is rendered, only cells that changed
/// from the previous frame are written to the terminal.
///
/// For wide (CJK/fullwidth) characters that occupy 2 columns, the first
/// column holds the character and `wide` is false. The second column
/// is a "continuation" cell with `wide = true`, which tells the renderer
/// not to write it separately (the terminal already rendered it as part
/// of the wide character).
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub char: char,
    pub fg: Color,
    pub bg: Color,
    pub style: Style,
    /// Whether this cell is the continuation (second column) of a wide
    /// character. Continuation cells are skipped during rendering because
    /// the terminal already drew them as part of the wide character.
    pub wide: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            char: ' ',
            fg: Color::Default,
            bg: Color::Default,
            style: Style::default(),
            wide: false,
        }
    }
}

impl Cell {
    /// Create a cell with a character and default styling.
    pub fn char(ch: char) -> Self {
        Cell {
            char: ch,
            ..Self::default()
        }
    }

    /// Create a styled cell.
    pub fn styled(ch: char, fg: Color, bg: Color, style: Style) -> Self {
        Cell {
            char: ch,
            fg,
            bg,
            style,
            wide: false,
        }
    }

    /// Create a continuation cell for a wide character's second column.
    /// These cells are skipped during rendering since the terminal
    /// already drew them as part of the wide character.
    pub fn wide_continuation(fg: Color, bg: Color, style: Style) -> Self {
        Cell {
            char: ' ',
            fg,
            bg,
            style,
            wide: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_fg_escape_default() {
        assert_eq!(Color::Default.fg_escape(), "\x1b[39m");
    }

    #[test]
    fn test_color_fg_escape_standard() {
        // Standard colors 0-7 use ESC[30–37m
        assert_eq!(Color::Ansi(0).fg_escape(), "\x1b[30m"); // black
        assert_eq!(Color::Ansi(1).fg_escape(), "\x1b[31m"); // red
        assert_eq!(Color::Ansi(7).fg_escape(), "\x1b[37m"); // white
    }

    #[test]
    fn test_color_fg_escape_bright() {
        // Bright colors 8-15 use ESC[90–97m
        assert_eq!(Color::Ansi(8).fg_escape(), "\x1b[90m"); // bright black
        assert_eq!(Color::Ansi(14).fg_escape(), "\x1b[96m"); // bright cyan
    }

    #[test]
    fn test_color_fg_escape_256() {
        // 8-bit colors use ESC[38;5;Nm
        assert_eq!(Color::Ansi(208).fg_escape(), "\x1b[38;5;208m"); // orange
        assert_eq!(Color::Ansi(236).fg_escape(), "\x1b[38;5;236m"); // bg_dim
    }

    #[test]
    fn test_color_fg_escape_rgb() {
        assert_eq!(Color::Rgb(255, 0, 128).fg_escape(), "\x1b[38;2;255;0;128m");
    }

    #[test]
    fn test_color_bg_escape_default() {
        assert_eq!(Color::Default.bg_escape(), "\x1b[49m");
    }

    #[test]
    fn test_color_bg_escape_standard() {
        assert_eq!(Color::Ansi(1).bg_escape(), "\x1b[41m"); // red bg
    }

    #[test]
    fn test_color_bg_escape_bright() {
        assert_eq!(Color::Ansi(8).bg_escape(), "\x1b[100m"); // bright black bg
    }

    #[test]
    fn test_color_bg_escape_256() {
        assert_eq!(Color::Ansi(237).bg_escape(), "\x1b[48;5;237m");
    }

    #[test]
    fn test_color_bg_escape_rgb() {
        assert_eq!(Color::Rgb(10, 20, 30).bg_escape(), "\x1b[48;2;10;20;30m");
    }

    #[test]
    fn test_style_escape() {
        let s = Style {
            bold: true,
            dim: false,
            italic: true,
            underline: false,
            blink: false,
        };
        assert_eq!(s.escape(), "\x1b[1m\x1b[3m");
    }

    #[test]
    fn test_style_default_is_empty() {
        let s = Style::default();
        assert_eq!(s.escape(), "");
    }

    #[test]
    fn test_cell_default() {
        let c = Cell::default();
        assert_eq!(c.char, ' ');
        assert_eq!(c.fg, Color::Default);
        assert_eq!(c.bg, Color::Default);
    }

    #[test]
    fn test_cell_char() {
        let c = Cell::char('X');
        assert_eq!(c.char, 'X');
        assert_eq!(c.fg, Color::Default);
    }

    #[test]
    fn test_cell_styled() {
        let c = Cell::styled('A', Color::RED, Color::BLUE, Style::bold());
        assert_eq!(c.char, 'A');
        assert_eq!(c.fg, Color::Ansi(1));
        assert_eq!(c.bg, Color::Ansi(4));
        assert!(c.style.bold);
    }
}
