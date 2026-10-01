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
pub const ORANGE: &str = "\x1b[38;5;208m"; // Warning / notice accent (matches `Output::warning`)
pub const BRIGHT_YELLOW: &str = "\x1b[93m";
pub const BRIGHT_CYAN: &str = "\x1b[96m";

// ── Tool frame palette ─────────────────────────────────────────────────────
//
// The tool-call family (execution cards, result blocks, diffs and the
// confirmation prompt) shares one visual language: a single filled *title
// band* per block, an outlined gutter body, and status conveyed by a colored
// glyph rather than by tinting whole lines.
//
// 256-color SGR is the common denominator for the terminals this CLI targets
// (the UI crate deliberately has no terminal-capability dependency), and the
// indices below are chosen from the neutral/flat ramps that survive both
// light and dark themes: they are either background shades or saturated
// mid-lightness hues, never the pure primary 1..4 which theme authors remap
// aggressively.
pub const BG_BAND: &str = "\x1b[48;5;236m"; // Card title band (neutral charcoal)
pub const BG_PROMPT: &str = "\x1b[48;5;237m"; // Confirmation prompt band (one step lighter)

pub const FG_FAINT: &str = "\x1b[38;5;240m"; // Gutters, timestamps, "more lines" footers
pub const FG_MUTED: &str = "\x1b[38;5;245m"; // Secondary text (argument values, snippets)
pub const FG_BAND: &str = "\x1b[38;5;252m"; // Tool name on a band (needs contrast vs BG_BAND)
pub const FG_ACCENT: &str = "\x1b[38;5;39m"; // Tool identity, paths, result titles
pub const FG_CMD: &str = "\x1b[38;5;45m"; // Shell command text
pub const FG_OK: &str = "\x1b[38;5;41m"; // Success status / additions
pub const FG_ERR: &str = "\x1b[38;5;197m"; // Failure status / removals
pub const FG_WARN: &str = "\x1b[38;5;214m"; // Denied calls, warnings, the approval prompt

// Assistant prose: rendered in the terminal's *default* foreground so it stays
// legible against any user theme (the modern CLI convention). Emitted once,
// before the first streamed content chunk, to clear any residual styling left
// behind by the spinner, the thinking block, or ANSI sequences that happened to
// be present in the model's own output. This is the single hook for theming
// assistant text later.
pub const ASSISTANT_TEXT: &str = RESET;

// Thinking/reasoning chain colors
pub const THINK_COLOR: &str = "\x1b[35m"; // Magenta for thinking text
pub const THINK_COLOR_DIM: &str = "\x1b[38;5;97m"; // Dimmer magenta

// Background colors (subtle, for tool call highlighting)
#[deprecated(note = "renamed to BG_BAND — the tool card title band")]
pub const BG_DIM: &str = "\x1b[48;5;236m";
#[deprecated(note = "tool batch headers use the outlined frame in ui::frame")]
pub const BG_TOOL: &str = "\x1b[48;5;237m";
#[deprecated(note = "renamed to BG_PROMPT — the confirmation prompt band")]
pub const BG_WARN: &str = "\x1b[48;5;17m";

// Line fill: clears from cursor to end of line, filling with current background color
pub const FILL_EOL: &str = "\x1b[K";

// UI styling presets
pub const TITLE_COLOR: &str = CYAN; // For titles and headers
pub const BOX_COLOR: &str = BLUE; // For box borders and frames
pub const WARNING_COLOR: &str = ORANGE; // For warnings and alerts (matches `Output::warning`)
pub const ACCENT_COLOR: &str = MAGENTA; // For highlights and emphasis

// Special escape sequences
pub const CLEAR_SCREEN: &str = "\x1b[2J\x1b[H";

/// Spinner frames for the progress indicator (Braille patterns)
pub const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Clear the entire current line (used by spinner to erase previous frame)
pub const CLEAR_LINE: &str = "\x1b[2K";
