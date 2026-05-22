// ANSI escape codes for terminal styling.

// Style modifiers
pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";
pub const ITALIC: &str = "\x1b[3m";
pub const UNDERLINE: &str = "\x1b[4m";

// Standard foreground colors
pub const RED: &str = "\x1b[31m";
pub const GREEN: &str = "\x1b[32m";
pub const YELLOW: &str = "\x1b[33m";
pub const BLUE: &str = "\x1b[34m";
pub const MAGENTA: &str = "\x1b[35m";
pub const CYAN: &str = "\x1b[36m";
pub const WHITE: &str = "\x1b[37m";

// Bright / extended colors
pub const GRAY: &str = "\x1b[90m";
pub const ORANGE: &str = "\x1b[38;5;208m";
pub const BRIGHT_YELLOW: &str = "\x1b[93m";
pub const BRIGHT_CYAN: &str = "\x1b[96m";

// Thinking/reasoning chain colors
pub const THINK_COLOR: &str = "\x1b[35m"; // Magenta for thinking text
pub const THINK_COLOR_DIM: &str = "\x1b[38;5;97m"; // Dimmer magenta

// Background colors (subtle, for tool call highlighting)
pub const BG_DIM: &str = "\x1b[48;5;236m"; // Dark gray bg — tool call results
pub const BG_TOOL: &str = "\x1b[48;5;237m"; // Slightly lighter gray bg — tool headers
pub const BG_WARN: &str = "\x1b[48;5;17m"; // Dark navy bg — confirmation/warning headers

// Line fill: clears from cursor to end of line, filling with current background color
pub const FILL_EOL: &str = "\x1b[K";

// UI styling presets
pub const TITLE_COLOR: &str = CYAN; // For titles and headers
pub const BOX_COLOR: &str = BLUE; // For box borders and frames
pub const WARNING_COLOR: &str = YELLOW; // For warnings and alerts
pub const ACCENT_COLOR: &str = MAGENTA; // For highlights and emphasis

// Special escape sequences
pub const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";

/// Spinner frames for the progress indicator (Braille patterns)
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Clear the entire current line (used by spinner to erase previous frame)
pub const CLEAR_LINE: &str = "\x1b[2K";
