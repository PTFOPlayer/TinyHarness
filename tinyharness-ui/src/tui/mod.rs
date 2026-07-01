#[allow(
    clippy::too_many_arguments,
    clippy::collapsible_if,
    clippy::cast_lossless,
    clippy::new_without_default
)]
pub mod app;
#[allow(
    clippy::too_many_arguments,
    clippy::collapsible_if,
    clippy::cast_lossless,
    clippy::new_without_default
)]
pub mod backend;
#[allow(
    clippy::too_many_arguments,
    clippy::collapsible_if,
    clippy::cast_lossless,
    clippy::new_without_default
)]
pub mod cell;
#[allow(clippy::too_many_arguments)]
pub mod event;
pub mod layout;
#[allow(
    clippy::too_many_arguments,
    clippy::collapsible_if,
    clippy::cast_lossless
)]
pub mod screen;
pub mod terminal;
pub mod widget;
pub mod widgets {
    pub mod conversation;
    pub mod input_bar;
    pub mod sidebar;
    pub mod spinner;
    pub mod status_bar;
    pub mod tool_output;
}

// ── TUI Agent Integration Types ──────────────────────────────────────────────
//
// These types define the communication protocol between the TUI event loop
// (main thread) and the background agent task (tokio runtime).

/// Events sent FROM the background agent task TO the TUI.
///
/// These drive UI updates: streaming text, tool call notifications,
/// status changes, etc.
#[derive(Clone, Debug)]
pub enum TuiAgentEvent {
    /// The agent started streaming a response.
    StreamingStarted,
    /// A chunk of assistant text arrived during streaming.
    StreamingText(String),
    /// A chunk of thinking/reasoning text arrived during streaming.
    StreamingThinking(String),
    /// The agent finished streaming a response.
    StreamingDone,
    /// The agent encountered an error.
    Error(String),
    /// A tool call was made by the assistant.
    ToolCall { name: String, args_summary: String },
    /// A tool produced a result.
    ToolResult {
        name: String,
        content: String,
        is_error: bool,
    },
    /// The agent mode changed.
    ModeChanged(String),
    /// The model name changed.
    ModelChanged(String),
    /// Token usage was updated.
    TokenUpdate { count: u64, limit: Option<u64> },
    /// Cumulative session stats updated (tool calls, total tokens).
    StatsUpdate { tool_calls: u64, total_tokens: u64 },
    /// Context window warning triggered (70%+ or 90%+).
    ContextWarning { percentage: f64, critical: bool },
    /// A system/info message to display.
    SystemMessage(String),
    /// The agent is requesting user confirmation for a tool call.
    /// The TUI should show a confirmation prompt.
    /// `diff_preview` contains a plain-text unified diff to display (for edit/write).
    ConfirmTool {
        name: String,
        args_summary: String,
        needs_approval: bool,
        diff_preview: Option<String>,
    },
    /// The agent is asking a question (from the question signal tool).
    Question {
        question: String,
        answers: Vec<String>,
    },
    /// The agent loop has exited (clean shutdown).
    Done,
}

/// Actions sent FROM the TUI TO the background agent task.
///
/// These represent user interactions that the agent needs to know about.
#[derive(Clone, Debug)]
pub enum TuiUserAction {
    /// The user submitted a message (text entered in the input bar).
    SendMessage(String),
    /// The user responded to a tool confirmation prompt.
    ConfirmResponse { approved: bool, auto_accept: bool },
    /// The user answered a question from the question signal tool.
    QuestionAnswer(String),
    /// The user requested to quit.
    Quit,
    /// The user interrupted the current generation (Ctrl+C).
    Interrupt,
}

// Re-export key types at the module root for convenience
pub use app::{Focus, TuiApp, TuiGuard, TuiState, spawn_stdin_reader};
pub use backend::{Backend, StdioBackend, TestBackend};
pub use cell::{Cell, Color, Style};
pub use event::{Event, EventParser, Key, KeyEvent, Modifiers, MouseButton, MouseEvent};
pub use layout::{Constraint, Direction, Layout, Rect};
pub use screen::Screen;
pub use terminal::{Size, Terminal};
pub use widget::{Action, Widget};

pub use widgets::conversation::{ContextWarningLevel, ConversationLine, ConversationWidget};
pub use widgets::input_bar::InputBarWidget;
pub use widgets::sidebar::SidebarWidget;
pub use widgets::spinner::SpinnerWidget;
pub use widgets::status_bar::StatusBarWidget;
pub use widgets::tool_output::{ToolOutputWidget, ToolResult, ToolStatus};
