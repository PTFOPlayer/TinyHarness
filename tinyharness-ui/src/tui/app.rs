// ── TUI Application Loop ──────────────────────────────────────────────────────
//
// The main TUI application that owns all widgets, handles the event loop,
// renders frames, and diff-updates the terminal.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::time::Duration;

use super::TuiAgentEvent;
use super::backend::Backend;
use super::cell::{Cell, Style};
use super::event::{Event, EventParser, Key, KeyEvent, MouseButton, MouseEvent};
use super::layout::{Constraint, Direction, Layout, Rect};
use super::screen::Screen;
use super::terminal::Terminal;
use super::widget::{Action, Widget};
use super::widgets::conversation::{ContextWarningLevel, ConversationLine, ConversationWidget};
use super::widgets::input_bar::InputBarWidget;
use super::widgets::sidebar::SidebarWidget;
use super::widgets::status_bar::StatusBarWidget;
use super::widgets::tool_output::{ToolOutputWidget, ToolResult, ToolStatus};

// ── Focus management ────────────────────────────────────────────────────────

/// Which widget currently has keyboard focus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    InputBar,
    Conversation,
    ToolOutput,
    Sidebar,
    /// Interactive file browser in the sidebar structure section.
    Structure,
}

// ── Application state ────────────────────────────────────────────────────────

/// State exposed to the outside world (agent loop integration).
#[derive(Clone, Debug)]
pub struct TuiState {
    /// Current agent mode label.
    pub mode: String,
    /// Current model name.
    pub model_name: String,
    /// Whether the sidebar is visible.
    pub sidebar_visible: bool,
    /// Whether we're currently streaming a response.
    pub streaming: bool,
    /// Token usage info.
    pub token_count: Option<u64>,
    /// Token limit.
    pub token_limit: Option<u64>,
    /// Session name.
    pub session_name: String,
    /// Message count.
    pub message_count: usize,
}

impl Default for TuiState {
    fn default() -> Self {
        Self {
            mode: "agent".to_string(),
            model_name: String::new(),
            sidebar_visible: true,
            streaming: false,
            token_count: None,
            token_limit: None,
            session_name: "unnamed".to_string(),
            message_count: 0,
        }
    }
}

// ── TUI Application ──────────────────────────────────────────────────────────

/// The main TUI application.
///
/// Owns all widgets and manages the event/render loop. The application
/// reads events from a channel (fed by a stdin reader thread), routes
/// them to the appropriate widget, and renders the screen using diff-based
/// updates for flicker-free rendering.
///
/// It also receives `TuiAgentEvent` messages from a background agent task
/// (streaming text, tool calls, status updates) and sends `TuiUserAction`
/// messages back to the agent task (user input, confirmations).
pub struct TuiApp<B: Backend> {
    terminal: Terminal<B>,
    screen: Screen,
    prev_screen: Screen,

    // Widgets
    status_bar: StatusBarWidget,
    conversation: ConversationWidget,
    sidebar: SidebarWidget,
    input_bar: InputBarWidget,
    tool_output: ToolOutputWidget,

    // State
    focus: Focus,
    state: TuiState,
    running: bool,

    // Agent integration channels
    /// Channel to send user actions to the background agent task.
    user_action_tx: mpsc::Sender<super::TuiUserAction>,
    /// Channel to receive agent events from the background agent task.
    agent_event_rx: mpsc::Receiver<TuiAgentEvent>,

    // Streaming state
    /// Text accumulated during the current streaming response.
    streaming_text: String,
    /// Whether we're currently in streaming mode (receiving chunks from the agent).
    is_streaming: bool,
    /// Whether we're currently in a thinking phase (receiving thinking chunks).
    is_thinking: bool,
    /// Accumulated thinking text for the current thinking phase.
    thinking_text: String,
    /// Whether we're waiting for the user to confirm a tool call.
    confirming: bool,
    /// Stored answers for the current question (to resolve number selections).
    pending_question_answers: Vec<String>,
    /// Whether the help overlay is currently visible.
    help_visible: bool,
    /// Scroll offset for the help overlay (in lines).
    help_scroll: usize,
    /// Number of lines in the help content (cached to avoid recomputing).
    help_line_count: usize,
    /// Whether the tool output panel is visible (toggled with Ctrl+T).
    tool_output_visible: bool,
}

impl<B: Backend> TuiApp<B> {
    /// Create a new TUI application.
    ///
    /// Takes channels for communicating with the background agent task.
    /// The `user_action_tx` channel is used to send user actions (messages,
    /// confirmations) to the agent. The `agent_event_rx` channel receives
    /// agent events (streaming text, tool calls) for display.
    pub fn new(
        terminal: Terminal<B>,
        user_action_tx: mpsc::Sender<super::TuiUserAction>,
        agent_event_rx: mpsc::Receiver<TuiAgentEvent>,
    ) -> io::Result<Self> {
        let size = terminal.size();
        let width = size.cols;
        let height = size.rows;

        Ok(TuiApp {
            terminal,
            screen: Screen::new(width, height),
            prev_screen: Screen::new(width, height),

            status_bar: StatusBarWidget::new("agent", "unknown"),
            conversation: ConversationWidget::new(),
            sidebar: SidebarWidget::new(),
            input_bar: InputBarWidget::new("agent", "unknown"),
            tool_output: ToolOutputWidget::new(),

            focus: Focus::InputBar,
            state: TuiState::default(),
            running: true,

            user_action_tx,
            agent_event_rx,

            streaming_text: String::new(),
            is_streaming: false,
            is_thinking: false,
            thinking_text: String::new(),
            confirming: false,
            pending_question_answers: Vec::new(),
            help_visible: false,
            help_scroll: 0,
            help_line_count: 0,
            tool_output_visible: false,
        })
    }

    /// Get a reference to the application state.
    pub fn state(&self) -> &TuiState {
        &self.state
    }

    /// Get a mutable reference to the application state.
    pub fn state_mut(&mut self) -> &mut TuiState {
        &mut self.state
    }

    // ── Widget accessors ─────────────────────────────────────────────────

    /// Get a reference to the conversation widget.
    pub fn conversation(&self) -> &ConversationWidget {
        &self.conversation
    }

    /// Get a mutable reference to the conversation widget.
    pub fn conversation_mut(&mut self) -> ConversationMut<'_> {
        ConversationMut(&mut self.conversation)
    }

    /// Get a mutable reference to the sidebar widget.
    pub fn sidebar_mut(&mut self) -> &mut SidebarWidget {
        &mut self.sidebar
    }

    /// Get a mutable reference to the tool output widget.
    pub fn tool_output_mut(&mut self) -> &mut ToolOutputWidget {
        &mut self.tool_output
    }

    /// Get a mutable reference to the status bar widget.
    pub fn status_bar_mut(&mut self) -> &mut StatusBarWidget {
        &mut self.status_bar
    }

    /// Set command names and subcommand completions for the input bar's
    /// tab-completion feature. Should be called once after construction,
    /// before the event loop starts.
    pub fn set_command_completions(
        &mut self,
        command_names: Vec<String>,
        subcommands: HashMap<String, Vec<String>>,
    ) {
        self.input_bar
            .set_command_completions(command_names, subcommands);
    }

    // ── Update helpers ───────────────────────────────────────────────────

    /// Update all widgets from the current state.
    pub fn sync_from_state(&mut self) {
        self.status_bar
            .update_labels(&self.state.mode, &self.state.model_name);
        self.status_bar.set_session_name(&self.state.session_name);
        self.status_bar.set_message_count(self.state.message_count);
        if let Some(count) = self.state.token_count {
            self.status_bar
                .set_token_count(count, self.state.token_limit);
        }
        self.status_bar.set_streaming(self.state.streaming);

        self.input_bar
            .update_labels(&self.state.mode, &self.state.model_name);
        self.sidebar.visible = self.state.sidebar_visible;
    }

    /// Add a user message to the conversation.
    pub fn push_user_message(&mut self, text: &str) {
        self.conversation.push(ConversationLine::User {
            text: text.to_string(),
        });
        self.state.message_count += 1;
    }

    /// Add an assistant message to the conversation.
    pub fn push_assistant_message(&mut self, text: &str) {
        self.conversation.push(ConversationLine::Assistant {
            text: text.to_string(),
        });
    }

    /// Add a tool call to the conversation.
    pub fn push_tool_call(&mut self, name: &str, args_summary: &str) {
        self.conversation.push(ConversationLine::ToolCall {
            name: name.to_string(),
            args_summary: args_summary.to_string(),
        });
    }

    /// Add a tool result to both the conversation and the tool output widget.
    pub fn push_tool_result(&mut self, name: &str, content: &str, is_error: bool) {
        self.conversation.push(ConversationLine::ToolResult {
            name: name.to_string(),
            content: content.to_string(),
            is_error,
        });
        self.tool_output.push(ToolResult {
            name: name.to_string(),
            args_summary: String::new(),
            content: content.to_string(),
            is_error,
            collapsed: true,
            status: if is_error {
                ToolStatus::Error {
                    message: content.to_string(),
                }
            } else {
                ToolStatus::Success { duration_ms: 0 }
            },
        });
    }

    /// Add a system message to the conversation.
    pub fn push_system_message(&mut self, text: &str) {
        self.conversation.push(ConversationLine::System {
            text: text.to_string(),
        });
    }

    /// Add a thinking chain to the conversation.
    pub fn push_thinking(&mut self, text: &str) {
        self.conversation.push(ConversationLine::Thinking {
            text: text.to_string(),
        });
    }

    /// Add a separator line to the conversation.
    pub fn push_separator(&mut self) {
        self.conversation.push(ConversationLine::Separator);
    }

    /// Add a confirmation prompt to the conversation.
    pub fn push_confirm_prompt(&mut self, name: &str, args_summary: &str) {
        self.conversation.push(ConversationLine::ConfirmPrompt {
            name: name.to_string(),
            args_summary: args_summary.to_string(),
            diff_preview: None,
        });
    }

    /// Set the streaming state (shows spinner in input bar, blocks input).
    pub fn set_streaming(&mut self, streaming: bool) {
        self.state.streaming = streaming;
        self.status_bar.set_streaming(streaming);
        self.input_bar.set_streaming(streaming);
    }

    // ── Layout ───────────────────────────────────────────────────────────

    /// Compute the layout for the current terminal size.
    ///
    /// Returns: (status_area, conv_area, sidebar_area, input_area, main_area, tool_output_area)
    fn compute_layout(&self) -> (Rect, Rect, Rect, Rect, Rect, Rect) {
        let size = self.terminal.size();
        let total = Rect::new(0, 0, size.cols, size.rows);

        // Input bar grows up to a maximum based on content but always keeps
        // room for the status bar, the conversation, and (optionally) the
        // tool output panel.
        let max_input_rows = (size.rows / 4).clamp(3, 10);
        let input_rows = self
            .input_bar
            .content_height(size.cols)
            .clamp(2, max_input_rows);

        // Vertical split: status bar | main area | input bar
        let vertical = Layout::new(Direction::Vertical).constraints(vec![
            Constraint::Length(1),          // status bar
            Constraint::Percentage(100),    // main area (takes remaining)
            Constraint::Length(input_rows), // input bar
        ]);
        let vertical_areas = vertical.split(total);
        let status_area = vertical_areas[0];
        let main_area = vertical_areas[1];
        let input_area = vertical_areas[2];

        // If tool output panel is visible, split main area vertically:
        // conversation (top 60%) | tool output (bottom 40%)
        let (conv_area, tool_output_area) = if self.tool_output_visible {
            let tool_split = Layout::new(Direction::Vertical).constraints(vec![
                Constraint::Percentage(60), // conversation
                Constraint::Percentage(40), // tool output
            ]);
            let split_areas = tool_split.split(main_area);
            (split_areas[0], split_areas[1])
        } else {
            (main_area, Rect::new(0, 0, 0, 0))
        };

        if self.state.sidebar_visible {
            // Horizontal split of main area: conversation | sidebar
            // The sidebar shares the full main area height (including tool output)
            let sidebar_width = self.sidebar.desired_width.min(size.cols / 2).max(15);
            let horizontal = Layout::new(Direction::Horizontal).constraints(vec![
                Constraint::Percentage(100),       // conversation
                Constraint::Length(sidebar_width), // sidebar
            ]);
            let horizontal_areas = horizontal.split(conv_area);
            let inner_conv = horizontal_areas[0];
            let sidebar_area = horizontal_areas[1];

            (
                status_area,
                inner_conv,
                sidebar_area,
                input_area,
                main_area,
                tool_output_area,
            )
        } else {
            // No sidebar — conversation takes the full main area
            (
                status_area,
                conv_area,
                Rect::new(0, 0, 0, 0),
                input_area,
                main_area,
                tool_output_area,
            )
        }
    }

    // ── Event handling ────────────────────────────────────────────────────

    /// Dismiss the help overlay (close and reset scroll).
    fn dismiss_help(&mut self) {
        self.help_visible = false;
        self.help_scroll = 0;
    }

    /// Handle a single event and return any action.
    fn handle_event(&mut self, event: &Event) -> Action {
        // If help overlay is visible, handle scrolling and dismiss keys
        if self.help_visible {
            if let Event::Key(key) = event {
                match key {
                    // Ctrl+H / F1 close the overlay
                    KeyEvent {
                        key: Key::Char('h'),
                        modifiers,
                    } if modifiers.ctrl => {
                        self.dismiss_help();
                        return Action::None;
                    }
                    KeyEvent {
                        key: Key::F(1),
                        modifiers,
                    } if !modifiers.ctrl && !modifiers.alt => {
                        self.dismiss_help();
                        return Action::None;
                    }
                    // Escape closes the overlay
                    KeyEvent {
                        key: Key::Escape, ..
                    } => {
                        self.dismiss_help();
                        return Action::None;
                    }
                    // Ctrl+C and Ctrl+D pass through (close help first, then fall through)
                    KeyEvent {
                        key: Key::Char('c'),
                        modifiers,
                    } if modifiers.ctrl => {
                        self.dismiss_help();
                        // Fall through to global handler below
                    }
                    KeyEvent {
                        key: Key::Char('d'),
                        modifiers,
                    } if modifiers.ctrl => {
                        self.dismiss_help();
                        // Fall through to global handler below
                    }
                    // Scroll up (Up arrow or 'k')
                    KeyEvent {
                        key: Key::Up,
                        modifiers,
                    } if !modifiers.ctrl && !modifiers.alt => {
                        self.help_scroll = self.help_scroll.saturating_sub(1);
                        return Action::None;
                    }
                    KeyEvent {
                        key: Key::Char('k'),
                        modifiers,
                    } if !modifiers.ctrl && !modifiers.alt => {
                        self.help_scroll = self.help_scroll.saturating_sub(1);
                        return Action::None;
                    }
                    // Scroll down (Down arrow or 'j')
                    KeyEvent {
                        key: Key::Down,
                        modifiers,
                    } if !modifiers.ctrl && !modifiers.alt => {
                        self.help_scroll = self.help_scroll.saturating_add(1);
                        return Action::None;
                    }
                    KeyEvent {
                        key: Key::Char('j'),
                        modifiers,
                    } if !modifiers.ctrl && !modifiers.alt => {
                        self.help_scroll = self.help_scroll.saturating_add(1);
                        return Action::None;
                    }
                    // Page up / Page down / Home / End
                    KeyEvent {
                        key: Key::PageUp, ..
                    } => {
                        self.help_scroll = self.help_scroll.saturating_sub(10);
                        return Action::None;
                    }
                    KeyEvent {
                        key: Key::PageDown, ..
                    } => {
                        self.help_scroll = self.help_scroll.saturating_add(10);
                        return Action::None;
                    }
                    KeyEvent { key: Key::Home, .. } => {
                        self.help_scroll = 0;
                        return Action::None;
                    }
                    KeyEvent { key: Key::End, .. } => {
                        let content_height = self.help_content_height();
                        let max_scroll = self.help_line_count.saturating_sub(content_height);
                        self.help_scroll = max_scroll;
                        return Action::None;
                    }
                    // Any other key dismisses the overlay
                    _ => {
                        self.dismiss_help();
                        return Action::None;
                    }
                }
            }
            // Allow mouse scroll events to pass through to the help overlay handler below
        }

        // Global keybindings (always active regardless of focus)
        if let Event::Key(key) = event {
            match key {
                // Ctrl+C: quit (or interrupt streaming)
                KeyEvent {
                    key: Key::Char('c'),
                    modifiers,
                } if modifiers.ctrl => {
                    if self.state.streaming {
                        self.set_streaming(false);
                        return Action::Interrupt;
                    }
                    return Action::Quit;
                }
                // Ctrl+D: quit
                KeyEvent {
                    key: Key::Char('d'),
                    modifiers,
                } if modifiers.ctrl => {
                    return Action::Quit;
                }
                // Ctrl+S: toggle sidebar
                KeyEvent {
                    key: Key::Char('s'),
                    modifiers,
                } if modifiers.ctrl => {
                    self.state.sidebar_visible = !self.state.sidebar_visible;
                    return Action::ToggleSidebar;
                }
                // Ctrl+P: focus structure (interactive file browser)
                KeyEvent {
                    key: Key::Char('p'),
                    modifiers,
                } if modifiers.ctrl => {
                    if self.state.sidebar_visible {
                        self.set_focus(Focus::Structure);
                    }
                    return Action::None;
                }
                // Ctrl+F: toggle search in conversation
                KeyEvent {
                    key: Key::Char('f'),
                    modifiers,
                } if modifiers.ctrl => {
                    self.conversation.toggle_search();
                    self.set_focus(if self.conversation.is_search_active() {
                        Focus::Conversation
                    } else {
                        Focus::InputBar
                    });
                    return Action::None;
                }
                // F1 or Ctrl+H: toggle help overlay
                KeyEvent {
                    key: Key::F(1),
                    modifiers,
                } if !modifiers.ctrl && !modifiers.alt => {
                    self.help_visible = !self.help_visible;
                    self.help_scroll = 0;
                    return Action::None;
                }
                KeyEvent {
                    key: Key::Char('h'),
                    modifiers,
                } if modifiers.ctrl => {
                    self.help_visible = !self.help_visible;
                    self.help_scroll = 0;
                    return Action::None;
                }
                // Ctrl+T: toggle tool output panel
                KeyEvent {
                    key: Key::Char('t'),
                    modifiers,
                } if modifiers.ctrl => {
                    self.tool_output_visible = !self.tool_output_visible;
                    if self.tool_output_visible {
                        self.tool_output.un_collapse_all();
                    }
                    return Action::None;
                }
                _ => {}
            }
        }

        // Resize events — update terminal size and screen buffers
        if let Event::Resize { cols, rows } = event {
            self.screen.resize(*cols, *rows);
            self.prev_screen.resize(*cols, *rows);
            self.terminal.update_size();
            // Clamp help scroll to valid range after resize
            if self.help_visible {
                let content_height = self.help_content_height();
                let max_scroll = self.help_line_count.saturating_sub(content_height);
                if self.help_scroll > max_scroll {
                    self.help_scroll = max_scroll;
                }
            }
            return Action::None;
        }

        // Mouse scroll events go to the widget under the mouse cursor,
        // not just the focused widget. This makes scroll feel natural.
        // If help overlay is visible, scroll that instead.
        if let Event::Mouse(MouseEvent::ScrollUp { row, col }) = event {
            if self.help_visible {
                self.help_scroll = self.help_scroll.saturating_sub(3);
                return Action::None;
            }
            self.handle_mouse_scroll(*row, *col, 3);
            return Action::None;
        }
        if let Event::Mouse(MouseEvent::ScrollDown { row, col }) = event {
            if self.help_visible {
                self.help_scroll = self.help_scroll.saturating_add(3);
                return Action::None;
            }
            self.handle_mouse_scroll(*row, *col, -3);
            return Action::None;
        }

        // Mouse click events: switch focus to the clicked widget
        if let Event::Mouse(MouseEvent::Press { row, col, button }) = event {
            return self.handle_mouse_click(*row, *col, *button);
        }

        // Scroll-related key events go to the focused scrollable widget
        if let Event::Key(key) = event {
            let sidebar_focused = matches!(self.focus, Focus::Sidebar | Focus::Structure);
            match key {
                KeyEvent {
                    key: Key::PageUp, ..
                } => {
                    if sidebar_focused {
                        self.sidebar.scroll_up(10)
                    } else {
                        self.conversation.scroll_up(20)
                    }
                    return Action::None;
                }
                KeyEvent {
                    key: Key::PageDown, ..
                } => {
                    if sidebar_focused {
                        self.sidebar.scroll_down(10)
                    } else {
                        self.conversation.scroll_down(20)
                    }
                    return Action::None;
                }
                KeyEvent { key: Key::Home, .. } => {
                    if sidebar_focused {
                        self.sidebar.scroll_home()
                    } else {
                        self.conversation.scroll_home()
                    }
                    return Action::None;
                }
                KeyEvent { key: Key::End, .. } => {
                    if !sidebar_focused {
                        self.conversation.scroll_to_bottom()
                    }
                    return Action::None;
                }
                KeyEvent {
                    key: Key::Up,
                    modifiers,
                } if modifiers.alt => {
                    self.conversation.scroll_up(3);
                    return Action::None;
                }
                KeyEvent {
                    key: Key::Down,
                    modifiers,
                } if modifiers.alt => {
                    self.conversation.scroll_down(3);
                    return Action::None;
                }
                _ => {}
            }
        }

        // Route other events to focused widget
        match self.focus {
            Focus::InputBar => {
                let action = self.input_bar.handle_event(event);
                if matches!(action, Action::SendMessage(_)) {
                    self.conversation.scroll_to_bottom();
                }
                action
            }
            Focus::Conversation => {
                // Conversation focus is only used for search mode.
                // When not searching, fall back to input bar behavior.
                if self.conversation.is_search_active() {
                    self.conversation.handle_event(event)
                } else {
                    // Search was closed — switch back to input bar
                    self.set_focus(Focus::InputBar);
                    self.input_bar.handle_event(event)
                }
            }
            Focus::ToolOutput => self.tool_output.handle_event(event),
            Focus::Sidebar => self.sidebar.handle_event(event),
            Focus::Structure => {
                // Tab/BackTab in structure mode exits it and cycles focus
                if let Event::Key(KeyEvent { key: Key::Tab, .. }) = event {
                    self.sidebar.exit_structure_mode();
                    self.cycle_focus(true);
                    return Action::None;
                }
                if let Event::Key(KeyEvent {
                    key: Key::BackTab, ..
                }) = event
                {
                    self.sidebar.exit_structure_mode();
                    self.cycle_focus(false);
                    return Action::None;
                }
                self.sidebar.handle_event(event)
            }
        }
    }

    /// Cycle focus between widgets.
    ///
    /// The conversation and input bar are treated as a unified unit —
    /// typing happens in the input bar and scroll keys always affect the
    /// conversation. The focus cycle is:
    ///   InputBar → ToolOutput (if visible) → Structure
    ///
    /// Conversation focus is only entered for search mode (Ctrl+F) and
    /// is automatically exited when search is closed.
    fn cycle_focus(&mut self, forward: bool) {
        let order: Vec<Focus> = if self.tool_output_visible {
            vec![Focus::InputBar, Focus::ToolOutput, Focus::Structure]
        } else {
            vec![Focus::InputBar, Focus::Structure]
        };
        let current = order.iter().position(|&f| f == self.focus).unwrap_or(0);
        let next = if forward {
            (current + 1) % order.len()
        } else {
            (current + order.len() - 1) % order.len()
        };
        self.set_focus(order[next]);
    }

    /// Set focus to a specific widget.
    ///
    /// When focusing InputBar, the conversation is still scrollable via
    /// scroll keys — they're treated as a unified unit. Conversation focus
    /// is only used for search mode.
    fn set_focus(&mut self, focus: Focus) {
        self.focus = focus;
        // Input bar is focused whenever we're in InputBar focus (unified with conversation)
        self.input_bar
            .set_focus(matches!(focus, Focus::InputBar | Focus::Conversation));
        // Update the status bar focus indicator
        let label = match focus {
            Focus::InputBar | Focus::Conversation => "input",
            Focus::ToolOutput => "tools",
            Focus::Sidebar | Focus::Structure => "files",
        };
        self.status_bar.set_focus_label(label);
        // When focusing the sidebar, automatically enter structure (file browser) mode
        if focus == Focus::Sidebar || focus == Focus::Structure {
            self.sidebar.visible = true;
            self.state.sidebar_visible = true;
            self.sidebar.enter_structure_mode();
        } else {
            self.sidebar.exit_structure_mode();
        }
    }

    // ── Mouse handling ──────────────────────────────────────────────────

    /// Handle a mouse click: switch focus to the clicked widget and
    /// perform widget-specific click actions (cursor positioning, etc.).
    fn handle_mouse_click(&mut self, row: u16, col: u16, button: MouseButton) -> Action {
        let (status_area, conv_area, sidebar_area, input_area, _main_area, _tool_area) =
            self.compute_layout();

        // Determine which widget area was clicked
        if Self::rect_contains(status_area, row, col) {
            // Click on status bar — no action, but don't unfocus
            return Action::None;
        }

        if Self::rect_contains(input_area, row, col) {
            // Click on input bar — focus it and position cursor
            if self.focus != Focus::InputBar {
                self.set_focus(Focus::InputBar);
            }
            if button == MouseButton::Left {
                self.input_bar.click_to_cursor(row, col, input_area);
            }
            return Action::None;
        }

        if self.state.sidebar_visible
            && !sidebar_area.is_empty()
            && Self::rect_contains(sidebar_area, row, col)
        {
            // Click on sidebar — always focus structure (file browser) mode
            if self.focus != Focus::Structure {
                self.set_focus(Focus::Structure);
            }
            // Handle the click to select/navigate entries
            if button == MouseButton::Left {
                self.sidebar.click_structure_entry(row, col, sidebar_area);
            }
            return Action::None;
        }

        if Self::rect_contains(conv_area, row, col) {
            // Click on conversation area — don't change focus.
            // The input bar and conversation are a unified unit; typing always
            // goes to the input bar while scroll keys always affect the conversation.
            // Only switch to conversation focus if search is active (for cursor
            // positioning in the search bar).
            if self.conversation.is_search_active() {
                if self.focus != Focus::Conversation {
                    self.set_focus(Focus::Conversation);
                }
            }
            return Action::None;
        }

        Action::None
    }

    /// Handle a mouse scroll event: scroll the widget under the mouse cursor.
    fn handle_mouse_scroll(&mut self, row: u16, _col: u16, delta: i32) {
        let (status_area, conv_area, sidebar_area, input_area, _main_area, _tool_area) =
            self.compute_layout();

        let n = delta.unsigned_abs() as usize;

        if Self::rect_contains(sidebar_area, row, 0) && self.state.sidebar_visible {
            // Scroll the sidebar
            if delta > 0 {
                self.sidebar.scroll_up(n);
            } else {
                self.sidebar.scroll_down(n);
            }
        } else if Self::rect_contains(conv_area, row, 0) {
            // Scroll the conversation
            if delta > 0 {
                self.conversation.scroll_up(n);
            } else {
                self.conversation.scroll_down(n);
            }
        }
        // Don't scroll status bar or input bar
        let _ = (status_area, input_area);
    }

    /// Check if a screen position (row, col) falls within a Rect.
    fn rect_contains(rect: Rect, row: u16, col: u16) -> bool {
        row >= rect.y && row < rect.y + rect.height && col >= rect.x && col < rect.x + rect.width
    }

    // ── Rendering ────────────────────────────────────────────────────────

    /// Render all widgets to the screen buffer.
    fn render_frame(&mut self) {
        let (status_area, conv_area, sidebar_area, input_area, _main_area, _tool_area) =
            self.compute_layout();

        // Clear the screen
        self.screen.clear();

        // Render widgets
        self.status_bar.render(status_area, &mut self.screen);
        self.conversation.render(conv_area, &mut self.screen);
        if self.state.sidebar_visible && !sidebar_area.is_empty() {
            self.sidebar.render(sidebar_area, &mut self.screen);
        }
        if self.tool_output_visible && !_tool_area.is_empty() {
            self.tool_output.render(_tool_area, &mut self.screen);
        }
        self.input_bar.render(input_area, &mut self.screen);

        // Render help overlay if visible
        if self.help_visible {
            self.render_help_overlay(conv_area);
        }
    }

    /// Returns the static help content lines as a slice.
    ///
    /// Extracted from `render_help_overlay` to avoid recreating the array
    /// every frame and to allow reuse for scroll calculations.
    fn help_content() -> &'static [&'static str] {
        &[
            "",
            "  Keyboard Shortcuts",
            "  ─────────────────────────────────────────────────",
            "",
            "  Global:",
            "    Ctrl+C         Quit (or interrupt streaming)",
            "    Ctrl+D          Quit",
            "    Ctrl+S          Toggle sidebar",
            "    Ctrl+T          Toggle tool output panel",
            "    Ctrl+P          Focus file browser",
            "    Ctrl+F          Search in conversation",
            "    Ctrl+H / F1     Toggle this help",
            "    Tab             Cycle focus forward",
            "    Shift+Tab       Cycle focus backward",
            "",
            "  Input Bar:",
            "    Enter           Send message",
            "    Shift+Enter     Insert newline",
            "    Escape          Clear input / Quit if empty",
            "    Up/Down         History navigation",
            "    Ctrl+A          Move to start of line",
            "    Ctrl+E          Move to end of line",
            "    Ctrl+U          Clear line before cursor",
            "    Ctrl+K          Clear line after cursor",
            "    Ctrl+W          Delete word backward",
            "    Ctrl+Y          Yank (paste) from kill ring",
            "    Ctrl+Left/Right Move by word",
            "    Alt+B / Alt+F   Move back/forward by word",
            "    Alt+Backspace   Delete word backward",
            "    Tab             Complete / command or cycle focus",
            "",
            "  Navigation:",
            "    PageUp/Down     Scroll conversation by page",
            "    Alt+Up/Down     Scroll conversation by 3 lines",
            "    Home/End        Scroll to top/bottom of conversation",
            "    Ctrl+F          Search in conversation",
            "    Escape          Close search",
            "",
            "  File Browser (sidebar):",
            "    Up/Down         Move selection",
            "    Enter           Enter directory",
            "    Escape          Go back / exit",
            "    PageUp/Down     Scroll by page",
            "    Home/End         First/last entry",
            "    /               Filter files by name",
            "",
            "  Confirm Prompt:",
            "    y               Approve tool call",
            "    n               Deny tool call",
            "    a               Approve all future calls",
            "    Escape          Deny",
            "",
        ]
    }

    /// Calculate the available content height for the help overlay based on
    /// the current terminal size. Returns 0 if there's not enough space.
    fn help_content_height(&self) -> usize {
        let size = self.terminal.size();
        // Available area: total height minus status bar (1) and input bar (3)
        let area_height = size.rows.saturating_sub(4); // 1 status + 3 input
        // Reserve 2 lines for top/bottom borders, 1 for the hint row
        let content_height = area_height.saturating_sub(3);
        content_height as usize
    }

    /// Render the help overlay on top of the conversation area.
    ///
    /// Shows a centered box with keyboard shortcuts, drawn over whatever
    /// is currently rendered. Supports scrolling when the terminal is too
    /// small to show all content. Up/Down/PgUp/PgDn/Home/End scroll,
    /// Escape or Ctrl+H/F1 close, any other key dismisses.
    fn render_help_overlay(&mut self, area: Rect) {
        use super::cell::Color;

        let lines = Self::help_content();
        // Cache the line count for scroll clamping
        self.help_line_count = lines.len();

        let box_width = 52u16;
        // Don't render if there's no usable space
        if area.height < 4 {
            return;
        }

        // Reserve: 2 lines for top/bottom borders, 1 for hint row at bottom
        let max_content = area.height.saturating_sub(3) as usize; // 2 border + 1 hint
        let content_height = max_content.min(lines.len());
        if content_height == 0 {
            return;
        }

        // Reserve one extra row at the bottom for the dismiss/scroll hint
        let visible_content_lines = content_height.saturating_sub(1);
        if visible_content_lines == 0 {
            return;
        }

        let box_height = content_height as u16 + 2; // +2 for top/bottom border
        let box_x = area.x + (area.width.saturating_sub(box_width)) / 2;
        let box_y = area.y + (area.height.saturating_sub(box_height)) / 2;

        // Clamp scroll so we don't scroll past the end
        let max_scroll = lines.len().saturating_sub(visible_content_lines);
        if self.help_scroll > max_scroll {
            self.help_scroll = max_scroll;
        }

        let overlay_bg = Color::Ansi(235); // dark gray
        let border_fg = Color::Ansi(244);
        let title_fg = Color::YELLOW;
        let section_fg = Color::CYAN;
        let key_fg = Color::WHITE;
        let desc_fg = Color::Ansi(252);
        let dim_fg = Color::Ansi(244);

        // Draw background fill
        let box_rect = Rect::new(box_x, box_y, box_width, box_height);
        self.screen.fill_rect(
            box_rect,
            Cell {
                char: ' ',
                fg: desc_fg,
                bg: overlay_bg,
                style: Style::default(),
                wide: false,
            },
        );

        // Draw border using the screen's draw_box method
        self.screen
            .draw_box(box_rect, border_fg, overlay_bg, Style::default());

        // Draw text lines (scrolled)
        let content_x = box_x + 1;
        let content_width = (box_width as usize).saturating_sub(2);

        for (i, line) in lines
            .iter()
            .skip(self.help_scroll)
            .take(visible_content_lines)
            .enumerate()
        {
            let row = box_y + 1 + i as u16;
            if row >= box_y + box_height - 2 {
                // Leave room for the hint row
                break;
            }

            let (fg, style) = if line.starts_with("  Keyboard Shortcuts") {
                (title_fg, Style::bold())
            } else if line.starts_with("  ────") {
                (border_fg, Style::default())
            } else if line.starts_with("  Global:")
                || line.starts_with("  Input Bar:")
                || line.starts_with("  Conversation:")
                || line.starts_with("  File Browser")
                || line.starts_with("  Confirm Prompt:")
            {
                (section_fg, Style::bold())
            } else if line.contains("Ctrl+")
                || line.contains("Shift+")
                || line.contains("Alt+")
                || line.contains("Tab")
                || line.contains("Enter")
                || line.contains("Escape")
                || line.contains("PageUp")
                || line.contains("Home")
                || line.contains("End")
                || line.contains("Up/Down")
                || line.contains("/")
            {
                (key_fg, Style::default())
            } else {
                (desc_fg, Style::default())
            };

            let display = if line.len() > content_width {
                use crate::tui::widget::truncate_str;
                format!("{}…", truncate_str(line, content_width.saturating_sub(1)))
            } else {
                line.to_string()
            };

            self.screen
                .write_str(row, content_x, &display, fg, overlay_bg, style);
        }

        // Draw dismiss/scroll hint at the bottom of the box (inside the border)
        let hint_row = box_y + box_height - 2;
        let can_scroll_up = self.help_scroll > 0;
        let can_scroll_down = self.help_scroll < max_scroll;
        let hint = if can_scroll_up || can_scroll_down {
            "  ↑↓ scroll · Esc to close"
        } else {
            "  Press Esc to close"
        };
        self.screen
            .write_str(hint_row, content_x, hint, dim_fg, overlay_bg, Style::dim());

        // Draw scroll position indicator inside the right border area
        // (placed between the border and content, not overlapping the border)
        if can_scroll_up || can_scroll_down {
            let scroll_char = if can_scroll_up && can_scroll_down {
                '↕'
            } else if can_scroll_up {
                '↑'
            } else {
                '↓'
            };
            let indicator_y = box_y + box_height / 2;
            // Draw inside the right border: column box_x + box_width - 2
            // (one column inside from the right border │)
            if let Some(cell) = self.screen.get_mut(indicator_y, box_x + box_width - 2) {
                cell.char = scroll_char;
                cell.wide = false;
                cell.fg = border_fg;
                cell.bg = overlay_bg;
                cell.style = Style::default();
            }
        }
    }

    /// Diff the current screen against the previous frame and write changes.
    ///
    /// Uses `Screen::diff_from` and `Screen::render_diff` which correctly
    /// handle wide (CJK/fullwidth) characters and cursor position tracking.
    fn flush_diff(&mut self) -> io::Result<()> {
        let width = self.screen.width();
        let ops = self.screen.diff_from(&self.prev_screen);
        let output = Screen::render_diff(&ops, width);

        if !output.is_empty() {
            self.terminal.write_all(output.as_bytes())?;
        }

        // Swap buffers
        std::mem::swap(&mut self.screen, &mut self.prev_screen);

        self.terminal.flush()?;
        Ok(())
    }

    // ── Main loop ────────────────────────────────────────────────────────

    /// Run the TUI event loop.
    ///
    /// This method takes ownership and blocks until the user quits.
    /// It reads events from the provided receiver (which should be fed
    /// by a stdin reader thread), and also processes events from the
    /// background agent task (streaming text, tool calls, etc.).
    pub fn run(&mut self, event_rx: mpsc::Receiver<Event>) -> io::Result<()> {
        // Enter raw mode and alternate screen
        self.terminal.enter_raw_mode()?;
        self.terminal.enter_alternate_screen()?;
        self.terminal.hide_cursor()?;
        self.terminal.enable_mouse()?;
        self.terminal.enable_bracketed_paste()?;

        // Initial render
        self.render_frame();
        self.flush_diff()?;

        while self.running {
            // Poll for UI events with a short timeout for smooth animation
            let event = event_rx.recv_timeout(Duration::from_millis(50));

            match event {
                Ok(ev) => {
                    let action = self.handle_event(&ev);
                    self.handle_action(action);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // No UI event
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    // Stdin event source closed — exit
                    break;
                }
            }

            // Process all pending agent events (non-blocking)
            while let Ok(agent_event) = self.agent_event_rx.try_recv() {
                self.handle_agent_event(agent_event);
            }

            // Update spinner animation
            if self.state.streaming {
                self.input_bar.tick_streaming();
            }

            // Render and flush
            self.render_frame();
            self.flush_diff()?;
        }

        // Cleanup
        self.terminal.disable_bracketed_paste()?;
        self.terminal.disable_mouse()?;
        self.terminal.show_cursor()?;
        self.terminal.leave_alternate_screen()?;
        self.terminal.leave_raw_mode()?;

        Ok(())
    }

    /// Handle an action returned by a widget.
    fn handle_action(&mut self, action: Action) {
        match action {
            Action::Quit => {
                self.running = false;
                let _ = self.user_action_tx.send(super::TuiUserAction::Quit);
            }
            Action::SendMessage(text) => {
                self.push_user_message(&text);
                // Send the message to the background agent task
                let _ = self
                    .user_action_tx
                    .send(super::TuiUserAction::SendMessage(text));
            }
            Action::ToggleSidebar => {
                self.state.sidebar_visible = !self.state.sidebar_visible;
            }
            Action::SwitchMode(mode) => {
                self.state.mode = mode;
                self.sync_from_state();
            }
            Action::CycleFocusForward => {
                self.cycle_focus(true);
            }
            Action::CycleFocusBackward => {
                self.cycle_focus(false);
            }
            Action::ScrollUp => {
                self.conversation.scroll_up(3);
            }
            Action::ScrollDown => {
                self.conversation.scroll_down(3);
            }
            Action::PageUp => {
                self.conversation.scroll_up(20);
            }
            Action::PageDown => {
                self.conversation.scroll_down(20);
            }
            Action::ConfirmYes => {
                self.confirming = false;
                self.input_bar.set_confirming(false);
                let _ = self
                    .user_action_tx
                    .send(super::TuiUserAction::ConfirmResponse {
                        approved: true,
                        auto_accept: false,
                    });
            }
            Action::ConfirmNo => {
                self.confirming = false;
                self.input_bar.set_confirming(false);
                let _ = self
                    .user_action_tx
                    .send(super::TuiUserAction::ConfirmResponse {
                        approved: false,
                        auto_accept: false,
                    });
            }
            Action::ConfirmAll => {
                self.confirming = false;
                self.input_bar.set_confirming(false);
                let _ = self
                    .user_action_tx
                    .send(super::TuiUserAction::ConfirmResponse {
                        approved: true,
                        auto_accept: true,
                    });
            }
            Action::AnswerQuestion(input) => {
                self.input_bar.set_questioning(false, 0);
                // Resolve the answer: if it's a number, map to the predefined answers
                let answer = if let Ok(num) = input.parse::<usize>() {
                    if num >= 1 && num <= self.pending_question_answers.len() {
                        self.pending_question_answers[num - 1].clone()
                    } else {
                        input
                    }
                } else {
                    input
                };
                self.pending_question_answers.clear();
                let _ = self
                    .user_action_tx
                    .send(super::TuiUserAction::QuestionAnswer(answer));
            }
            Action::Interrupt => {
                let _ = self.user_action_tx.send(super::TuiUserAction::Interrupt);
            }
            Action::ExitStructureMode => {
                // Exit structure mode — return focus to input bar
                self.set_focus(Focus::InputBar);
            }
            Action::None => {}
        }
    }

    /// Finalize any in-progress thinking block, updating the conversation
    /// with the accumulated thinking text and clearing internal state.
    fn finalize_thinking(&mut self) {
        if self.is_thinking {
            self.is_thinking = false;
            if let Some(ConversationLine::Thinking { text: t }) = self.conversation.last_mut() {
                *t = self.thinking_text.clone();
            }
            self.thinking_text.clear();
        }
    }

    /// Process an agent event received from the background task.
    fn handle_agent_event(&mut self, event: TuiAgentEvent) {
        match event {
            TuiAgentEvent::StreamingStarted => {
                self.is_streaming = true;
                self.streaming_text.clear();
                self.is_thinking = false;
                self.thinking_text.clear();
                self.input_bar.set_streaming_label("Thinking");
                self.input_bar.set_streaming(true);
                self.set_streaming(true);
                // Don't push a Thinking placeholder here — push it lazily
                // when the first StreamingThinking event arrives. This avoids
                // showing an empty thinking indicator when the model produces
                // no thinking content (e.g., non-Ollama models or thinking disabled).
            }
            TuiAgentEvent::StreamingText(text) => {
                // If we were thinking, finalize the thinking block first
                let was_thinking = self.is_thinking;
                self.finalize_thinking();
                if was_thinking {
                    self.input_bar.set_streaming_label("Responding");
                }
                self.streaming_text.push_str(&text);
                // Update the last assistant message or add a new one
                self.update_streaming_assistant_message();
                // Ensure we auto-scroll to follow the new content
                self.conversation.scroll_to_bottom();
            }
            TuiAgentEvent::StreamingThinking(text) => {
                // If this is the first thinking chunk, push a blank line for
                // visual separation from the previous message, then push a
                // Thinking line lazily. This mirrors the CLI behavior of writing
                // a newline before the [thinking] header.
                if !self.is_thinking {
                    self.is_thinking = true;
                    self.conversation.push(ConversationLine::Separator);
                    self.conversation.push(ConversationLine::Thinking {
                        text: String::new(),
                    });
                }
                self.thinking_text.push_str(&text);
                // Update the thinking indicator in the conversation
                if let Some(ConversationLine::Thinking { text: t }) = self.conversation.last_mut() {
                    // Show a brief preview of thinking content
                    let preview = if self.thinking_text.len() > 80 {
                        use crate::tui::widget::truncate_str;
                        format!("{}…", truncate_str(&self.thinking_text, 78))
                    } else {
                        self.thinking_text.clone()
                    };
                    *t = preview;
                }
                // Ensure we auto-scroll to follow thinking content
                self.conversation.scroll_to_bottom();
            }
            TuiAgentEvent::StreamingDone => {
                self.finalize_thinking();
                // Finalize the streaming text as a complete assistant message
                if !self.streaming_text.is_empty() {
                    // The streaming text was already being displayed incrementally
                    // via update_streaming_assistant_message, so just finalize
                    self.streaming_text.clear();
                }
                self.is_streaming = false;
                self.set_streaming(false);
            }
            TuiAgentEvent::Error(msg) => {
                self.finalize_thinking();
                self.push_system_message(&format!("⚠ Error: {}", msg));
                self.is_streaming = false;
                self.set_streaming(false);
            }
            TuiAgentEvent::ToolCall { name, args_summary } => {
                self.push_tool_call(&name, &args_summary);
            }
            TuiAgentEvent::ToolResult {
                name,
                content,
                is_error,
            } => {
                self.push_tool_result(&name, &content, is_error);
            }
            TuiAgentEvent::ModeChanged(mode) => {
                self.state.mode = mode;
                self.sync_from_state();
            }
            TuiAgentEvent::ModelChanged(model) => {
                self.state.model_name = model;
                self.sync_from_state();
            }
            TuiAgentEvent::TokenUpdate { count, limit } => {
                self.state.token_count = Some(count);
                self.state.token_limit = limit;
                self.status_bar.set_token_count(count, limit);
            }
            TuiAgentEvent::ContextWarning {
                percentage,
                critical,
            } => {
                let level = if critical {
                    ContextWarningLevel::Critical(percentage)
                } else {
                    ContextWarningLevel::Warning(percentage)
                };
                self.conversation.set_context_warning(level);
            }
            TuiAgentEvent::SystemMessage(msg) => {
                self.push_system_message(&msg);
            }
            TuiAgentEvent::ConfirmTool {
                name,
                args_summary,
                needs_approval: _,
                diff_preview,
            } => {
                // Show a confirmation prompt in the conversation and switch
                // the input bar to confirmation mode. The agent loop will
                // block until we send a ConfirmResponse back.
                self.conversation.push(ConversationLine::ConfirmPrompt {
                    name: name.clone(),
                    args_summary: args_summary.clone(),
                    diff_preview: diff_preview.clone(),
                });
                self.confirming = true;
                self.input_bar.set_confirming(true);
                self.set_focus(Focus::InputBar);
            }
            TuiAgentEvent::Question { question, answers } => {
                // Show the question in the conversation with styled answers.
                // The user can type a number or custom text, then press Enter.
                self.conversation.push(ConversationLine::Question {
                    question: question.clone(),
                    answers: answers.clone(),
                });
                // Enter question mode in the input bar
                self.input_bar.set_questioning(true, answers.len());
                self.set_focus(Focus::InputBar);
                // Store the answers so we can resolve number selections
                self.pending_question_answers = answers;
            }
            TuiAgentEvent::Done => {
                self.finalize_thinking();
                self.is_streaming = false;
                self.set_streaming(false);
            }
        }
    }

    /// Update the streaming assistant message in the conversation widget.
    ///
    /// If the last conversation line is an assistant message being streamed,
    /// update its text. Otherwise, add a new assistant message.
    fn update_streaming_assistant_message(&mut self) {
        // Check if the last line is an assistant message we can update
        if self.conversation.last_is_assistant() && self.is_streaming {
            // Update the last assistant message with accumulated streaming text
            if let Some(ConversationLine::Assistant { text }) = self.conversation.last_mut() {
                *text = self.streaming_text.clone();
            }
        } else {
            // Add a new assistant message
            self.conversation.push(ConversationLine::Assistant {
                text: self.streaming_text.clone(),
            });
        }
    }
}

// ── RAII guard for terminal state ────────────────────────────────────────────

/// RAII guard that ensures the terminal is restored when dropped,
/// even if the TUI crashes or panics.
pub struct TuiGuard<B: Backend> {
    terminal: Option<Terminal<B>>,
}

impl<B: Backend> TuiGuard<B> {
    /// Create a guard from a terminal. The terminal will be restored
    /// when the guard is dropped.
    pub fn new(terminal: Terminal<B>) -> Self {
        Self {
            terminal: Some(terminal),
        }
    }

    /// Take the terminal out of the guard, releasing the restore obligation.
    pub fn take(mut self) -> Terminal<B> {
        self.terminal.take().unwrap()
    }
}

impl<B: Backend> Drop for TuiGuard<B> {
    fn drop(&mut self) {
        if let Some(ref mut terminal) = self.terminal {
            let _ = terminal.disable_bracketed_paste();
            let _ = terminal.disable_mouse();
            let _ = terminal.show_cursor();
            let _ = terminal.leave_alternate_screen();
            let _ = terminal.leave_raw_mode();
        }
    }
}

// ── Conversation mut helper (newtype to avoid lifetime issues) ───────────────

/// A helper that provides mutable access to the conversation widget.
pub struct ConversationMut<'a>(pub &'a mut ConversationWidget);

// ── Stdin event reader ───────────────────────────────────────────────────────

/// Spawns a background thread that reads raw bytes from stdin and parses
/// them into events, sending them through a channel.
///
/// Also spawns a SIGWINCH signal handler thread that detects terminal resizes
/// and injects `Event::Resize` events into the same channel.
pub fn spawn_stdin_reader() -> (mpsc::Sender<Event>, mpsc::Receiver<Event>) {
    let (tx, rx) = mpsc::channel();

    // Stdin reader thread
    let stdin_tx = tx.clone();
    std::thread::spawn(move || {
        let mut parser = EventParser::new();
        let mut buf = [0u8; 64];

        loop {
            match std::io::stdin().read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(n) => {
                    parser.feed(&buf[..n]);
                    while let Some(event) = parser.parse() {
                        if stdin_tx.send(event).is_err() {
                            return; // Receiver dropped
                        }
                    }
                }
                Err(_) => break,
            }
        }
    });

    // SIGWINCH handler thread — detects terminal resize and sends Resize events
    let resize_tx = tx.clone();
    std::thread::spawn(move || {
        #[cfg(unix)]
        {
            use super::terminal::Size;
            use std::sync::atomic::{AtomicBool, Ordering as SignalOrdering};

            let resize_flag = std::sync::Arc::new(AtomicBool::new(false));
            // Register SIGWINCH handler — clones the Arc so we can still read the flag
            if signal_hook::flag::register(signal_hook::consts::SIGWINCH, resize_flag.clone())
                .is_err()
            {
                // Could not register signal handler — resize detection disabled
                return;
            }

            loop {
                std::thread::sleep(Duration::from_millis(100));
                if resize_flag.swap(false, SignalOrdering::SeqCst) {
                    if let Ok(size) = Size::from_terminal() {
                        let _ = resize_tx.send(Event::Resize {
                            cols: size.cols,
                            rows: size.rows,
                        });
                    }
                }
            }
        }

        #[cfg(not(unix))]
        {
            let _ = resize_tx;
            loop {
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });

    (tx, rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::backend::TestBackend;
    use crate::tui::event::Modifiers;
    use crate::tui::terminal::Size;

    fn make_app() -> TuiApp<TestBackend> {
        let backend = TestBackend::new(Size::new(80, 24));
        let mut terminal = Terminal::new(backend).unwrap();
        // `Terminal::new` discovers size from the real terminal/env vars and ignores the writer's
        // size. Pin it explicitly so layout- and scroll-dependent assertions don't flake on
        // terminals of unexpected size.
        terminal.set_size(Size::new(80, 24));
        let (user_action_tx, _user_action_rx) = mpsc::channel();
        let (_, agent_event_rx) = mpsc::channel();
        TuiApp::new(terminal, user_action_tx, agent_event_rx).unwrap()
    }

    #[test]
    fn test_app_new() {
        let app = make_app();
        assert!(app.running);
        assert_eq!(app.focus, Focus::InputBar);
    }

    #[test]
    fn test_app_default_state() {
        let app = make_app();
        assert_eq!(app.state.mode, "agent");
        assert!(app.state.sidebar_visible);
        assert!(!app.state.streaming);
    }

    #[test]
    fn test_app_push_user_message() {
        let mut app = make_app();
        app.push_user_message("Hello");
        assert_eq!(app.state.message_count, 1);
    }

    #[test]
    fn test_app_push_assistant_message() {
        let mut app = make_app();
        app.push_assistant_message("Hi there!");
    }

    #[test]
    fn test_app_push_tool_call() {
        let mut app = make_app();
        app.push_tool_call("read", "src/main.rs");
    }

    #[test]
    fn test_app_push_tool_result() {
        let mut app = make_app();
        app.push_tool_result("read", "fn main() {}", false);
    }

    #[test]
    fn test_app_push_system_message() {
        let mut app = make_app();
        app.push_system_message("Session started");
    }

    #[test]
    fn test_app_push_thinking() {
        let mut app = make_app();
        app.push_thinking("Let me think...");
    }

    #[test]
    fn test_app_set_streaming() {
        let mut app = make_app();
        app.set_streaming(true);
        assert!(app.state.streaming);
        app.set_streaming(false);
        assert!(!app.state.streaming);
    }

    #[test]
    fn test_app_sync_from_state() {
        let mut app = make_app();
        app.state.mode = "planning".to_string();
        app.state.model_name = "gpt-4".to_string();
        app.state.session_name = "test-session".to_string();
        app.state.message_count = 5;
        app.sync_from_state();
    }

    #[test]
    fn test_app_cycle_focus() {
        let mut app = make_app();
        // Cycle: InputBar → Structure → InputBar (without tool output)
        assert_eq!(app.focus, Focus::InputBar);
        app.cycle_focus(true);
        assert_eq!(app.focus, Focus::Structure);
        app.cycle_focus(true);
        assert_eq!(app.focus, Focus::InputBar);
        app.cycle_focus(false);
        assert_eq!(app.focus, Focus::Structure);
    }

    #[test]
    fn test_app_set_focus() {
        let mut app = make_app();
        // InputBar focus keeps the input bar focused
        app.set_focus(Focus::InputBar);
        assert!(app.input_bar.focused());
        // Conversation focus also keeps input bar focused (unified)
        app.set_focus(Focus::Conversation);
        assert!(app.input_bar.focused());
        // Structure focus removes input bar focus
        app.set_focus(Focus::Structure);
        assert!(!app.input_bar.focused());
    }

    #[test]
    fn test_app_compute_layout() {
        let app = make_app();
        let (status, conv, sidebar, input, _main, _tool) = app.compute_layout();
        assert_eq!(status.height, 1);
        // Input bar is now at least 2 rows: top border + one content row.
        assert!(input.height >= 2);
        assert!(conv.height > 0);
        assert!(sidebar.width > 0);
    }

    #[test]
    fn test_app_compute_layout_no_sidebar() {
        let mut app = make_app();
        app.state.sidebar_visible = false;
        let (_status, conv, sidebar, _input, _main, _tool) = app.compute_layout();
        assert!(conv.width > 0);
        assert_eq!(sidebar.width, 0);
    }

    #[test]
    fn test_app_handle_quit() {
        let mut app = make_app();
        let event = Event::Key(KeyEvent {
            key: Key::Char('c'),
            modifiers: Modifiers::ctrl(),
        });
        let action = app.handle_event(&event);
        app.handle_action(action);
        assert!(!app.running);
    }

    #[test]
    fn test_app_handle_ctrl_d() {
        let mut app = make_app();
        let event = Event::Key(KeyEvent {
            key: Key::Char('d'),
            modifiers: Modifiers::ctrl(),
        });
        let action = app.handle_event(&event);
        app.handle_action(action);
        assert!(!app.running);
    }

    #[test]
    fn test_app_handle_toggle_sidebar() {
        let mut app = make_app();
        let event = Event::Key(KeyEvent {
            key: Key::Char('s'),
            modifiers: Modifiers::ctrl(),
        });
        app.handle_event(&event);
        assert!(!app.state.sidebar_visible);
        app.handle_event(&event);
        assert!(app.state.sidebar_visible);
    }

    #[test]
    fn test_app_handle_tab_focus() {
        let mut app = make_app();
        // Tab on empty input returns CycleFocusForward from input bar
        let event = Event::Key(KeyEvent {
            key: Key::Tab,
            modifiers: Modifiers::new(),
        });
        let action = app.handle_event(&event);
        // The action should cycle focus forward (InputBar → Structure)
        app.handle_action(action);
        assert_eq!(app.focus, Focus::Structure);
    }

    #[test]
    fn test_app_render_frame() {
        let mut app = make_app();
        app.push_user_message("Hello");
        app.push_assistant_message("Hi!");
        app.render_frame();
        // Should not panic
    }

    #[test]
    fn test_tui_state_default() {
        let state = TuiState::default();
        assert_eq!(state.mode, "agent");
        assert!(state.sidebar_visible);
        assert!(!state.streaming);
    }

    #[test]
    fn test_focus_default() {
        assert_eq!(Focus::default(), Focus::InputBar);
    }

    #[test]
    fn test_spawn_stdin_reader() {
        // Just verify the channels are created
        let (_tx, rx) = spawn_stdin_reader();
        // Drop the sender to close the thread
        drop(_tx);
        // Receiver should eventually get a Disconnected error
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    // ── Mouse tests ────────────────────────────────────────────────────

    #[test]
    fn test_app_rect_contains() {
        let rect = Rect::new(5, 3, 20, 10);
        assert!(TuiApp::<TestBackend>::rect_contains(rect, 3, 5)); // top-left
        assert!(TuiApp::<TestBackend>::rect_contains(rect, 12, 24)); // bottom-right
        assert!(!TuiApp::<TestBackend>::rect_contains(rect, 2, 5)); // above
        assert!(!TuiApp::<TestBackend>::rect_contains(rect, 13, 5)); // below
        assert!(!TuiApp::<TestBackend>::rect_contains(rect, 3, 4)); // left
        assert!(!TuiApp::<TestBackend>::rect_contains(rect, 3, 25)); // right
    }

    #[test]
    fn test_app_mouse_click_conversation() {
        let mut app = make_app();
        assert_eq!(app.focus, Focus::InputBar);
        // Click in the conversation area — should NOT change focus
        // (input bar and conversation are a unified unit)
        let (_, conv_area, _, _, _, _) = app.compute_layout();
        let event = Event::Mouse(MouseEvent::Press {
            row: conv_area.y,
            col: conv_area.x,
            button: MouseButton::Left,
        });
        app.handle_event(&event);
        assert_eq!(app.focus, Focus::InputBar);
    }

    #[test]
    fn test_app_mouse_click_input_bar() {
        let mut app = make_app();
        // Click in the input bar area (near bottom) — should focus input bar
        let (_, _, _, input_area, _, _) = app.compute_layout();
        let input_row = input_area.y + 1; // middle of input bar
        let event = Event::Mouse(MouseEvent::Press {
            row: input_row,
            col: input_area.x,
            button: MouseButton::Left,
        });
        app.handle_event(&event);
        assert_eq!(app.focus, Focus::InputBar);
    }

    #[test]
    fn test_app_mouse_click_sidebar() {
        let mut app = make_app();
        assert!(app.state.sidebar_visible);
        // Click in the sidebar area (rightmost columns) — should focus Structure
        let (_, _, sidebar_area, _, _, _) = app.compute_layout();
        if !sidebar_area.is_empty() {
            let sidebar_row = sidebar_area.y + 2;
            let sidebar_col = sidebar_area.x + 2;
            let event = Event::Mouse(MouseEvent::Press {
                row: sidebar_row,
                col: sidebar_col,
                button: MouseButton::Left,
            });
            app.handle_event(&event);
            assert_eq!(app.focus, Focus::Structure);
            assert!(app.sidebar.is_structure_mode());
        }
    }

    #[test]
    fn test_app_mouse_scroll_conversation() {
        let mut app = make_app();
        for i in 0..30 {
            app.push_user_message(&format!("Message {}", i));
        }
        // Scroll up in the conversation area
        let (_, conv_area, _, _, _, _) = app.compute_layout();
        let conv_row = conv_area.y + 5;
        let event = Event::Mouse(MouseEvent::ScrollUp {
            row: conv_row,
            col: conv_area.x,
        });
        app.handle_event(&event);
        // Verify the event was handled (no panic) and scroll was applied
        // by scrolling down and checking it doesn't crash
        let event2 = Event::Mouse(MouseEvent::ScrollDown {
            row: conv_row,
            col: conv_area.x,
        });
        app.handle_event(&event2);
    }

    #[test]
    fn test_app_mouse_scroll_sidebar() {
        let mut app = make_app();
        app.sidebar.structure = (0..30).map(|i| format!("file_{}.rs", i)).collect();
        // Scroll up in the sidebar area
        let (_, _, sidebar_area, _, _, _) = app.compute_layout();
        if !sidebar_area.is_empty() {
            let sidebar_row = sidebar_area.y + 3;
            let event = Event::Mouse(MouseEvent::ScrollUp {
                row: sidebar_row,
                col: sidebar_area.x + 2,
            });
            app.handle_event(&event);
        }
    }

    // ── Feature 3: Ctrl+C sends Interrupt ──────────────────────────────

    #[test]
    fn test_ctrl_c_sends_interrupt_while_streaming() {
        let mut app = make_app();
        app.state.streaming = true;
        let event = Event::Key(KeyEvent {
            key: Key::Char('c'),
            modifiers: Modifiers::ctrl(),
        });
        let action = app.handle_event(&event);
        assert!(matches!(action, Action::Interrupt));
        assert!(!app.state.streaming);
    }

    #[test]
    fn test_ctrl_c_quits_when_not_streaming() {
        let mut app = make_app();
        assert!(!app.state.streaming);
        let event = Event::Key(KeyEvent {
            key: Key::Char('c'),
            modifiers: Modifiers::ctrl(),
        });
        let action = app.handle_event(&event);
        assert!(matches!(action, Action::Quit));
    }

    #[test]
    fn test_interrupt_action_sends_user_action() {
        let mut app = make_app();
        app.handle_action(Action::Interrupt);
        // Should not panic
    }

    // ── Feature 2: Help overlay ────────────────────────────────────────

    #[test]
    fn test_help_overlay_toggle_ctrl_h() {
        let mut app = make_app();
        assert!(!app.help_visible);
        let event = Event::Key(KeyEvent {
            key: Key::Char('h'),
            modifiers: Modifiers::ctrl(),
        });
        app.handle_event(&event);
        assert!(app.help_visible);
        app.handle_event(&event);
        assert!(!app.help_visible);
    }

    #[test]
    fn test_help_overlay_toggle_f1() {
        let mut app = make_app();
        let event = Event::Key(KeyEvent {
            key: Key::F(1),
            modifiers: Modifiers::new(),
        });
        app.handle_event(&event);
        assert!(app.help_visible);
        // Any key dismisses
        let dismiss = Event::Key(KeyEvent {
            key: Key::Char('a'),
            modifiers: Modifiers::new(),
        });
        app.handle_event(&dismiss);
        assert!(!app.help_visible);
    }

    #[test]
    fn test_help_overlay_dismisses_on_any_key() {
        let mut app = make_app();
        app.help_visible = true;
        let event = Event::Key(KeyEvent {
            key: Key::Enter,
            modifiers: Modifiers::new(),
        });
        app.handle_event(&event);
        assert!(!app.help_visible);
    }

    #[test]
    fn test_help_overlay_renders() {
        let mut app = make_app();
        app.help_visible = true;
        app.render_frame();
        // Should not panic
    }

    #[test]
    fn test_help_overlay_scroll_up_down() {
        let mut app = make_app();
        app.help_visible = true;
        assert_eq!(app.help_scroll, 0);
        // Scroll down
        let event = Event::Key(KeyEvent {
            key: Key::Down,
            modifiers: Modifiers::new(),
        });
        app.handle_event(&event);
        assert_eq!(app.help_scroll, 1);
        app.handle_event(&event);
        assert_eq!(app.help_scroll, 2);
        // Scroll up
        let event = Event::Key(KeyEvent {
            key: Key::Up,
            modifiers: Modifiers::new(),
        });
        app.handle_event(&event);
        assert_eq!(app.help_scroll, 1);
        // Can't scroll past top
        app.help_scroll = 0;
        app.handle_event(&event);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn test_help_overlay_scroll_page() {
        let mut app = make_app();
        app.help_visible = true;
        assert_eq!(app.help_scroll, 0);
        // Page down
        let event = Event::Key(KeyEvent {
            key: Key::PageDown,
            modifiers: Modifiers::new(),
        });
        app.handle_event(&event);
        assert_eq!(app.help_scroll, 10);
        // Page up
        let event = Event::Key(KeyEvent {
            key: Key::PageUp,
            modifiers: Modifiers::new(),
        });
        app.handle_event(&event);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn test_help_overlay_home_end() {
        let mut app = make_app();
        app.help_visible = true;
        assert_eq!(app.help_scroll, 0);
        // Scroll down first
        app.help_scroll = 5;
        // Home
        let event = Event::Key(KeyEvent {
            key: Key::Home,
            modifiers: Modifiers::new(),
        });
        app.handle_event(&event);
        assert_eq!(app.help_scroll, 0);
        // End — scroll to bottom
        // First trigger a render to populate help_line_count
        app.render_frame();
        let event = Event::Key(KeyEvent {
            key: Key::End,
            modifiers: Modifiers::new(),
        });
        app.handle_event(&event);
        // help_scroll should be set to max_scroll (clamped to content)
        // In a 24-row terminal, with status(1)+input(3)=4, area=20,
        // content_height ≈ 17, help_content has 52 lines, so max_scroll ≈ 35
        // Just verify scroll moved forward from 0
        assert!(app.help_scroll > 0);
    }

    #[test]
    fn test_help_overlay_escape_dismisses() {
        let mut app = make_app();
        app.help_visible = true;
        app.help_scroll = 5;
        let event = Event::Key(KeyEvent {
            key: Key::Escape,
            modifiers: Modifiers::new(),
        });
        app.handle_event(&event);
        assert!(!app.help_visible);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn test_help_overlay_mouse_scroll() {
        let mut app = make_app();
        app.help_visible = true;
        assert_eq!(app.help_scroll, 0);
        // Scroll down with mouse
        let event = Event::Mouse(MouseEvent::ScrollDown { row: 5, col: 5 });
        app.handle_event(&event);
        assert_eq!(app.help_scroll, 3);
        // Scroll up with mouse
        let event = Event::Mouse(MouseEvent::ScrollUp { row: 5, col: 5 });
        app.handle_event(&event);
        assert_eq!(app.help_scroll, 0);
    }

    #[test]
    fn test_help_content_static() {
        let lines = TuiApp::<TestBackend>::help_content();
        assert!(!lines.is_empty());
        assert!(lines.len() > 30); // Should have all shortcut sections
    }

    // ── Feature 4: Tool output panel ───────────────────────────────────

    #[test]
    fn test_tool_output_toggle_ctrl_t() {
        let mut app = make_app();
        assert!(!app.tool_output_visible);
        let event = Event::Key(KeyEvent {
            key: Key::Char('t'),
            modifiers: Modifiers::ctrl(),
        });
        app.handle_event(&event);
        assert!(app.tool_output_visible);
        app.handle_event(&event);
        assert!(!app.tool_output_visible);
    }

    #[test]
    fn test_tool_output_layout_with_panel_visible() {
        let mut app = make_app();
        app.tool_output_visible = true;
        let (.., tool_area) = app.compute_layout();
        assert!(!tool_area.is_empty());
        assert!(tool_area.height > 0);
    }

    #[test]
    fn test_tool_output_layout_without_panel() {
        let app = make_app();
        assert!(!app.tool_output_visible);
        let (.., tool_area) = app.compute_layout();
        assert!(tool_area.is_empty());
    }

    #[test]
    fn test_tool_output_cycle_focus_includes_tool_output() {
        let mut app = make_app();
        app.tool_output_visible = true;
        // Cycle: InputBar → ToolOutput → Structure → InputBar
        app.cycle_focus(true);
        assert_eq!(app.focus, Focus::ToolOutput);
        app.cycle_focus(true);
        assert_eq!(app.focus, Focus::Structure);
        app.cycle_focus(true);
        assert_eq!(app.focus, Focus::InputBar);
    }

    #[test]
    fn test_tool_output_renders_when_visible() {
        let mut app = make_app();
        app.tool_output_visible = true;
        app.tool_output.push(ToolResult {
            name: "read".to_string(),
            args_summary: "src/main.rs".to_string(),
            content: "fn main() {}".to_string(),
            is_error: false,
            collapsed: true,
            status: ToolStatus::Success { duration_ms: 42 },
        });
        app.render_frame();
        // Should not panic
    }

    // ── Input bar editing shortcuts tests are in input_bar.rs ───────────
    // (they access private fields directly)
}
