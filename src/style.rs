/// ANSI escape codes for terminal styling.

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

// Bright / extended colors
pub const GRAY: &str = "\x1b[90m";
pub const ORANGE: &str = "\x1b[38;5;208m";

// Special escape sequences
pub const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";
