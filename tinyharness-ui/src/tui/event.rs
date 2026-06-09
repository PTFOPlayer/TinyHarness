// ── Event system for the TUI ──────────────────────────────────────────────────
//
// Defines keyboard, mouse, and resize events, plus a cross-platform
// event reader that parses raw terminal input into structured events.

use std::io;
use std::time::Duration;

// ── Key and modifier types ─────────────────────────────────────────────────

/// A keyboard key press event.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
}

/// A keyboard key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A printable character.
    Char(char),
    /// Enter/Return key.
    Enter,
    /// Escape key.
    Escape,
    /// Backspace key.
    Backspace,
    /// Delete key.
    Delete,
    /// Tab key.
    Tab,
    /// Backtab (Shift+Tab).
    BackTab,
    /// Arrow keys.
    Up,
    Down,
    Left,
    Right,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Page Up key.
    PageUp,
    /// Page Down key.
    PageDown,
    /// Insert key.
    Insert,
    /// Function keys F1–F12.
    F(u8),
}

/// Keyboard modifier flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Modifiers {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn ctrl() -> Self {
        Self {
            ctrl: true,
            ..Self::default()
        }
    }

    pub fn alt() -> Self {
        Self {
            alt: true,
            ..Self::default()
        }
    }

    pub fn shift() -> Self {
        Self {
            shift: true,
            ..Self::default()
        }
    }
}

// ── Mouse events ─────────────────────────────────────────────────────────────

/// A mouse event.
#[derive(Clone, Debug, PartialEq)]
pub enum MouseEvent {
    /// Mouse button pressed.
    Press {
        row: u16,
        col: u16,
        button: MouseButton,
    },
    /// Mouse button released.
    Release { row: u16, col: u16 },
    /// Mouse wheel scrolled up.
    ScrollUp { row: u16, col: u16 },
    /// Mouse wheel scrolled down.
    ScrollDown { row: u16, col: u16 },
    /// Mouse moved (hover/drag).
    Move { row: u16, col: u16 },
}

/// Mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

// ── Top-level event enum ────────────────────────────────────────────────────

/// An event from the terminal.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A key was pressed.
    Key(KeyEvent),
    /// A mouse event occurred.
    Mouse(MouseEvent),
    /// The terminal was resized.
    Resize { cols: u16, rows: u16 },
    /// A paste event (bracketed paste mode).
    Paste(String),
}

// ── Event parser ─────────────────────────────────────────────────────────────

/// Parses raw bytes from stdin into events.
///
/// Terminal input arrives as a stream of bytes. Escape sequences can be
/// multi-byte (e.g., `\x1b[A` for Up arrow, `\x1b[1;5C` for Ctrl+Right).
/// The parser accumulates bytes until a complete sequence is recognized.
#[derive(Default)]
pub struct EventParser {
    buf: Vec<u8>,
}

impl EventParser {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Feed bytes into the parser buffer.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Try to parse a complete event from the buffer.
    ///
    /// Returns `Some(event)` if a complete event was parsed, `None` if
    /// more bytes are needed. Incomplete sequences remain in the buffer.
    pub fn parse(&mut self) -> Option<Event> {
        if self.buf.is_empty() {
            return None;
        }

        // Check for escape sequences
        if self.buf[0] == 0x1b && self.buf.len() > 1 {
            return self.parse_escape_sequence();
        }

        // Single byte: control character or regular key
        let byte = self.buf[0];
        self.buf.drain(..1); // Consume only the first byte

        match byte {
            // Control characters
            0x0D | 0x0A => Some(Event::Key(KeyEvent {
                key: Key::Enter,
                modifiers: Modifiers::new(),
            })),
            0x09 => Some(Event::Key(KeyEvent {
                key: Key::Tab,
                modifiers: Modifiers::new(),
            })),
            0x7F => Some(Event::Key(KeyEvent {
                key: Key::Backspace,
                modifiers: Modifiers::new(),
            })),
            0x1B => Some(Event::Key(KeyEvent {
                key: Key::Escape,
                modifiers: Modifiers::new(),
            })),
            // Ctrl+A through Ctrl+Z (0x01 through 0x1A)
            0x01..=0x1A => Some(Event::Key(KeyEvent {
                key: Key::Char((b'a' + (byte - 0x01)) as char),
                modifiers: Modifiers::ctrl(),
            })),
            // Regular printable character
            byte if byte >= 0x20 => Some(Event::Key(KeyEvent {
                key: Key::Char(byte as char),
                modifiers: Modifiers::new(),
            })),
            // Ignore other control characters
            _ => None,
        }
    }

    /// Parse an escape sequence starting with \x1b.
    fn parse_escape_sequence(&mut self) -> Option<Event> {
        let buf = &self.buf;

        // Bracketed paste: \x1b[200~ ... \x1b[201~
        if buf.len() >= 6 && &buf[0..6] == b"\x1b[200~" {
            // Find the end marker
            if let Some(end) = self.find_bracketed_paste_end() {
                let paste_content = String::from_utf8_lossy(&buf[6..end]).to_string();
                self.buf.drain(..end + 6);
                return Some(Event::Paste(paste_content));
            }
            // Need more data
            return None;
        }

        // CSI sequences: \x1b[ ...
        if buf.len() >= 2 && buf[1] == b'[' {
            return self.parse_csi_sequence();
        }

        // Alt + key: \x1b<key>
        if buf.len() >= 2 {
            let alt_key = buf[1];
            self.buf.drain(..2);
            match alt_key {
                0x0D | 0x0A => Some(Event::Key(KeyEvent {
                    key: Key::Enter,
                    modifiers: Modifiers::alt(),
                })),
                0x7F => Some(Event::Key(KeyEvent {
                    key: Key::Backspace,
                    modifiers: Modifiers::alt(),
                })),
                byte if byte >= 0x20 => Some(Event::Key(KeyEvent {
                    key: Key::Char(byte as char),
                    modifiers: Modifiers::alt(),
                })),
                _ => None,
            }
        } else {
            // Just escape — might be a standalone Escape key
            // Wait a bit for more bytes; if none come, it's Escape
            None
        }
    }

    /// Parse a CSI (Control Sequence Introducer) sequence.
    fn parse_csi_sequence(&mut self) -> Option<Event> {
        // Find the terminating byte (0x40–0x7E for standard CSI)
        let end_pos = self.buf[2..]
            .iter()
            .position(|&b| (0x40..=0x7E).contains(&b))
            .map(|p| p + 2);

        let end = match end_pos {
            Some(p) => p,
            None => {
                // Need more bytes
                // But if the buffer is getting long and still no terminator,
                // it might be a malformed sequence. Give up after 20 bytes.
                if self.buf.len() > 20 {
                    self.buf.drain(..1); // Remove the ESC
                    return None;
                }
                return None;
            }
        };

        // Extract the parameter bytes and final byte before draining
        let params = self.buf[2..end].to_vec();
        let final_byte = self.buf[end];

        // Consume the entire sequence
        self.buf.drain(..=end);

        // Parse the sequence
        self.dispatch_csi(&params, final_byte)
    }

    /// Dispatch a parsed CSI sequence to the appropriate event.
    fn dispatch_csi(&self, params: &[u8], final_byte: u8) -> Option<Event> {
        // Parse parameter string (semicolon-separated numbers)
        // For SGR mouse events, the format is `\x1b[<button;col;rowM`
        // The `<` (0x3C) is part of the SGR mouse extension and appears
        // as the first byte of the parameter string. We detect this and
        // strip it so that the first number parses correctly.
        let param_str = std::str::from_utf8(params).unwrap_or("");
        let is_sgr_mouse = param_str.starts_with('<');
        let clean_str = if is_sgr_mouse {
            &param_str[1..]
        } else {
            param_str
        };
        let nums: Vec<u16> = clean_str
            .split(';')
            .filter_map(|s| s.parse().ok())
            .collect();

        let mods = Self::parse_modifiers(nums.get(1));

        match final_byte {
            // Arrow keys and special keys (VT100)
            b'A' => Some(Event::Key(KeyEvent {
                key: Key::Up,
                modifiers: mods,
            })),
            b'B' => Some(Event::Key(KeyEvent {
                key: Key::Down,
                modifiers: mods,
            })),
            b'C' => Some(Event::Key(KeyEvent {
                key: Key::Right,
                modifiers: mods,
            })),
            b'D' => Some(Event::Key(KeyEvent {
                key: Key::Left,
                modifiers: mods,
            })),
            b'H' => Some(Event::Key(KeyEvent {
                key: Key::Home,
                modifiers: mods,
            })),
            b'F' => Some(Event::Key(KeyEvent {
                key: Key::End,
                modifiers: mods,
            })),

            // Function keys
            b'P' => Some(Event::Key(KeyEvent {
                key: Key::F(1),
                modifiers: mods,
            })),
            b'Q' => Some(Event::Key(KeyEvent {
                key: Key::F(2),
                modifiers: mods,
            })),
            b'R' => Some(Event::Key(KeyEvent {
                key: Key::F(3),
                modifiers: mods,
            })),
            b'S' => Some(Event::Key(KeyEvent {
                key: Key::F(4),
                modifiers: mods,
            })),

            // Extended keys (with parameters)
            b'~' => match nums.first().copied().unwrap_or(0) {
                1 => Some(Event::Key(KeyEvent {
                    key: Key::Home,
                    modifiers: mods,
                })),
                2 => Some(Event::Key(KeyEvent {
                    key: Key::Insert,
                    modifiers: mods,
                })),
                3 => Some(Event::Key(KeyEvent {
                    key: Key::Delete,
                    modifiers: mods,
                })),
                4 => Some(Event::Key(KeyEvent {
                    key: Key::End,
                    modifiers: mods,
                })),
                5 => Some(Event::Key(KeyEvent {
                    key: Key::PageUp,
                    modifiers: mods,
                })),
                6 => Some(Event::Key(KeyEvent {
                    key: Key::PageDown,
                    modifiers: mods,
                })),
                11..=15 => Some(Event::Key(KeyEvent {
                    key: Key::F((nums[0] - 10) as u8),
                    modifiers: mods,
                })),
                17..=21 => Some(Event::Key(KeyEvent {
                    key: Key::F((nums[0] - 11) as u8),
                    modifiers: mods,
                })),
                23 => Some(Event::Key(KeyEvent {
                    key: Key::F(12),
                    modifiers: mods,
                })),
                _ => None,
            },

            // Mouse events (SGR mode: \x1b[<button;col;rowM or \x1b[<button;col;rowm)
            b'M' => self.parse_xterm_mouse(&nums, false),
            b'm' => self.parse_xterm_mouse(&nums, true),

            _ => None,
        }
    }

    /// Parse modifier flags from CSI parameter.
    ///
    /// CSI modifier encoding:
    /// - No modifier field → no modifiers
    /// - 2 = Shift
    /// - 3 = Alt (Meta)
    /// - 5 = Ctrl
    /// - 6 = Ctrl+Shift
    fn parse_modifiers(mod_code: Option<&u16>) -> Modifiers {
        let code = mod_code.copied().unwrap_or(0);
        Modifiers {
            shift: code == 2 || code == 4 || code == 6 || code == 8,
            alt: code == 3 || code == 7,
            ctrl: code == 5 || code == 6 || code == 7 || code == 8,
        }
    }

    /// Parse an xterm SGR mouse event.
    ///
    /// SGR mouse format: `\x1b[<button;col;rowM` or `\x1b[<button;col;rowm`
    /// The `<` prefix is part of the SGR mouse extension and gets included
    /// in the parameter string, causing the first number to fail parsing
    /// (e.g., `"<64"` instead of `"64"`). We strip the leading `<` here.
    fn parse_xterm_mouse(&self, nums: &[u16], release: bool) -> Option<Event> {
        if nums.len() < 3 {
            return None;
        }
        let button_code = nums[0];
        let col = nums[1].saturating_sub(1);
        let row = nums[2].saturating_sub(1);

        if release {
            Some(Event::Mouse(MouseEvent::Release { row, col }))
        } else {
            match button_code {
                0 => Some(Event::Mouse(MouseEvent::Press {
                    row,
                    col,
                    button: MouseButton::Left,
                })),
                1 => Some(Event::Mouse(MouseEvent::Press {
                    row,
                    col,
                    button: MouseButton::Middle,
                })),
                2 => Some(Event::Mouse(MouseEvent::Press {
                    row,
                    col,
                    button: MouseButton::Right,
                })),
                64 => Some(Event::Mouse(MouseEvent::ScrollUp { row, col })),
                65 => Some(Event::Mouse(MouseEvent::ScrollDown { row, col })),
                _ => None,
            }
        }
    }

    /// Find the end of a bracketed paste sequence.
    fn find_bracketed_paste_end(&self) -> Option<usize> {
        let end_marker = b"\x1b[201~";
        self.buf
            .windows(end_marker.len())
            .position(|w| w == end_marker)
    }
}

// ── Async event reader ───────────────────────────────────────────────────────

/// Reads events from stdin asynchronously.
///
/// Uses a background thread to read raw bytes from stdin and parse them
/// into events. Events are sent through a channel.
pub struct EventReader {
    #[allow(dead_code)]
    tx: std::sync::mpsc::Sender<Event>,
    rx: std::sync::mpsc::Receiver<Event>,
}

impl EventReader {
    /// Create a new event reader.
    ///
    /// Spawns a background thread that reads from stdin in raw mode
    /// and parses events.
    pub fn new() -> io::Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        Ok(Self { tx, rx })
    }

    /// Start reading events from stdin.
    ///
    /// This spawns a background thread that reads raw bytes from stdin
    /// and sends parsed events through the channel.
    pub fn start(&self) {
        // Event reading from stdin will be integrated with the
        // tokio event loop in the app. For now, the parser can be
        // used directly for testing.
    }

    /// Try to receive an event with a timeout.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Event> {
        self.rx.recv_timeout(timeout).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_regular_char() {
        let mut parser = EventParser::new();
        parser.feed(b"a");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Char('a'),
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_enter() {
        let mut parser = EventParser::new();
        parser.feed(b"\r");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Enter,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_tab() {
        let mut parser = EventParser::new();
        parser.feed(b"\t");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Tab,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_backspace() {
        let mut parser = EventParser::new();
        parser.feed(b"\x7f");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Backspace,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_ctrl_a() {
        let mut parser = EventParser::new();
        parser.feed(b"\x01");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Char('a'),
                modifiers: Modifiers::ctrl(),
            })
        );
    }

    #[test]
    fn test_parse_ctrl_c() {
        let mut parser = EventParser::new();
        parser.feed(b"\x03");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Char('c'),
                modifiers: Modifiers::ctrl(),
            })
        );
    }

    #[test]
    fn test_parse_arrow_up() {
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[A");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Up,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_arrow_down() {
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[B");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Down,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_arrow_right() {
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[C");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Right,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_arrow_left() {
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[D");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Left,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_ctrl_arrow_up() {
        let mut parser = EventParser::new();
        // \x1b[1;5A = Ctrl+Up
        parser.feed(b"\x1b[1;5A");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Up,
                modifiers: Modifiers {
                    ctrl: true,
                    alt: false,
                    shift: false
                },
            })
        );
    }

    #[test]
    fn test_parse_shift_arrow_up() {
        let mut parser = EventParser::new();
        // \x1b[1;2A = Shift+Up
        parser.feed(b"\x1b[1;2A");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Up,
                modifiers: Modifiers {
                    ctrl: false,
                    alt: false,
                    shift: true
                },
            })
        );
    }

    #[test]
    fn test_parse_alt_key() {
        let mut parser = EventParser::new();
        parser.feed(b"\x1ba");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Char('a'),
                modifiers: Modifiers::alt(),
            })
        );
    }

    #[test]
    fn test_parse_home() {
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[H");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Home,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_end() {
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[F");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::End,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_page_up() {
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[5~");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::PageUp,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_page_down() {
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[6~");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::PageDown,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_delete() {
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[3~");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Key(KeyEvent {
                key: Key::Delete,
                modifiers: Modifiers::new(),
            })
        );
    }

    #[test]
    fn test_parse_multiple_events() {
        let mut parser = EventParser::new();
        parser.feed(b"abc");
        let a = parser.parse().unwrap();
        let b = parser.parse().unwrap();
        let c = parser.parse().unwrap();
        assert_eq!(
            a,
            Event::Key(KeyEvent {
                key: Key::Char('a'),
                modifiers: Modifiers::new()
            })
        );
        assert_eq!(
            b,
            Event::Key(KeyEvent {
                key: Key::Char('b'),
                modifiers: Modifiers::new()
            })
        );
        assert_eq!(
            c,
            Event::Key(KeyEvent {
                key: Key::Char('c'),
                modifiers: Modifiers::new()
            })
        );
    }

    #[test]
    fn test_modifiers_ctrl() {
        let mods = Modifiers::ctrl();
        assert!(mods.ctrl);
        assert!(!mods.alt);
        assert!(!mods.shift);
    }

    #[test]
    fn test_modifiers_alt() {
        let mods = Modifiers::alt();
        assert!(!mods.ctrl);
        assert!(mods.alt);
        assert!(!mods.shift);
    }

    #[test]
    fn test_parse_sgr_mouse_scroll_up() {
        // SGR mouse format: \x1b[<64;col;rowM
        // Button 64 = scroll up
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[<64;10;5M");
        let event = parser.parse().unwrap();
        assert_eq!(event, Event::Mouse(MouseEvent::ScrollUp { row: 4, col: 9 }));
    }

    #[test]
    fn test_parse_sgr_mouse_scroll_down() {
        // SGR mouse format: \x1b[<65;col;rowM
        // Button 65 = scroll down
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[<65;10;5M");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Mouse(MouseEvent::ScrollDown { row: 4, col: 9 })
        );
    }

    #[test]
    fn test_parse_sgr_mouse_left_click() {
        // SGR mouse format: \x1b[<0;col;rowM
        let mut parser = EventParser::new();
        parser.feed(b"\x1b[<0;1;1M");
        let event = parser.parse().unwrap();
        assert_eq!(
            event,
            Event::Mouse(MouseEvent::Press {
                row: 0,
                col: 0,
                button: MouseButton::Left,
            })
        );
    }
}
