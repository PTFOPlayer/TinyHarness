// ── Terminal control primitives ─────────────────────────────────────────────
//
// Raw mode, alternate screen buffer, cursor control, and terminal size
// detection — all using raw ANSI sequences and platform-specific APIs.
// No external TUI framework dependency.

use std::io::{self, Write};

// ── Windows Console API FFI ─────────────────────────────────────────────────

#[cfg(windows)]
mod winapi {
    use std::ffi::c_void;
    use std::io;

    pub type Bool = i32;
    pub type DWORD = u32;
    pub type HANDLE = *mut c_void;

    pub const STD_INPUT_HANDLE: DWORD = 0xFFFFFFF6; // (DWORD)-10
    pub const STD_OUTPUT_HANDLE: DWORD = 0xFFFFFFF5; // (DWORD)-11

    pub const ENABLE_ECHO_INPUT: DWORD = 0x0004;
    pub const ENABLE_LINE_INPUT: DWORD = 0x0002;
    pub const ENABLE_PROCESSED_INPUT: DWORD = 0x0001;
    pub const ENABLE_VIRTUAL_TERMINAL_INPUT: DWORD = 0x0200;
    pub const ENABLE_VIRTUAL_TERMINAL_PROCESSING: DWORD = 0x0004;

    #[allow(non_snake_case)]
    #[repr(C)]
    pub struct CONSOLE_SCREEN_BUFFER_INFO {
        pub dwSize: COORD,
        pub dwCursorPosition: COORD,
        pub wAttributes: u16,
        pub srWindow: SMALL_RECT,
        pub dwMaximumWindowSize: COORD,
    }

    #[allow(non_snake_case)]
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct COORD {
        pub X: i16,
        pub Y: i16,
    }

    #[allow(non_snake_case)]
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct SMALL_RECT {
        pub Left: i16,
        pub Top: i16,
        pub Right: i16,
        pub Bottom: i16,
    }

    unsafe extern "system" {
        pub fn GetStdHandle(nStdHandle: DWORD) -> HANDLE;
        pub fn GetConsoleMode(hConsoleHandle: HANDLE, lpMode: *mut DWORD) -> Bool;
        pub fn SetConsoleMode(hConsoleHandle: HANDLE, dwMode: DWORD) -> Bool;
        pub fn GetConsoleScreenBufferInfo(
            hConsoleOutput: HANDLE,
            lpConsoleScreenBufferInfo: *mut CONSOLE_SCREEN_BUFFER_INFO,
        ) -> Bool;
    }

    /// Get the console mode for the given standard handle.
    pub fn get_console_mode(handle: HANDLE) -> io::Result<DWORD> {
        let mut mode: DWORD = 0;
        unsafe {
            if GetConsoleMode(handle, &mut mode) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(mode)
    }

    /// Set the console mode for the given standard handle.
    pub fn set_console_mode(handle: HANDLE, mode: DWORD) -> io::Result<()> {
        unsafe {
            if SetConsoleMode(handle, mode) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(())
    }

    /// Get the stdin handle.
    pub fn stdin_handle() -> HANDLE {
        unsafe { GetStdHandle(STD_INPUT_HANDLE) }
    }

    /// Get the stdout handle.
    pub fn stdout_handle() -> HANDLE {
        unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }
    }
}

// ── Terminal size ────────────────────────────────────────────────────────────

/// Terminal size in columns and rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub cols: u16,
    pub rows: u16,
}

impl Size {
    pub fn new(cols: u16, rows: u16) -> Self {
        Self { cols, rows }
    }

    /// Get the terminal size from the OS.
    ///
    /// On Unix, uses TIOCGWINSZ ioctl. On Windows, uses the Console API.
    #[cfg(unix)]
    pub fn from_terminal() -> io::Result<Self> {
        // Safety: TIOCGWINSZ is a read-only ioctl that doesn't modify memory.
        let mut winsize: libc::winsize = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let result = unsafe {
            libc::ioctl(
                libc::STDOUT_FILENO,
                libc::TIOCGWINSZ,
                &mut winsize as *mut _,
            )
        };
        if result == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(Size {
            rows: winsize.ws_row,
            cols: winsize.ws_col,
        })
    }

    /// Get the terminal size from the Windows Console API.
    #[cfg(windows)]
    pub fn from_terminal() -> io::Result<Self> {
        use self::winapi::*;
        let handle = stdout_handle();
        if handle.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "GetStdHandle failed"));
        }
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = unsafe { std::mem::zeroed() };
        unsafe {
            if GetConsoleScreenBufferInfo(handle, &mut info) == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        Ok(Size {
            cols: (info.srWindow.Right - info.srWindow.Left + 1) as u16,
            rows: (info.srWindow.Bottom - info.srWindow.Top + 1) as u16,
        })
    }

    /// Fallback for platforms without a native terminal size API.
    #[cfg(not(any(unix, windows)))]
    pub fn from_terminal() -> io::Result<Self> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "terminal size detection not supported on this platform",
        ))
    }

    /// Fallback: use environment variables COLUMNS and LINES.
    pub fn from_env() -> Option<Self> {
        let cols = std::env::var("COLUMNS")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())?;
        let rows = std::env::var("LINES")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())?;
        Some(Size { cols, rows })
    }

    /// Default size used when detection fails.
    pub fn default_size() -> Self {
        Size { cols: 80, rows: 24 }
    }
}

// ── Raw mode control ────────────────────────────────────────────────────────

/// Saved terminal state for restoration on exit (Unix).
#[cfg(unix)]
struct SavedTermios {
    original: libc::termios,
}

/// Saved console mode for restoration on exit (Windows).
#[cfg(windows)]
struct SavedConsoleMode {
    stdin_mode: u32,
}

/// Manages raw terminal mode and alternate screen buffer.
///
/// On construction, the current terminal settings are saved.
/// `enter_raw_mode()` switches to raw mode (no echo, no line buffering,
/// no signal processing). `leave_raw_mode()` restores the original settings.
///
/// The alternate screen buffer is managed separately:
/// `enter_alternate_screen()` switches to a private buffer so the original
/// terminal content is preserved on exit.
pub struct Terminal<W: Write> {
    writer: W,
    size: Size,
    #[cfg(unix)]
    saved_termios: Option<SavedTermios>,
    #[cfg(windows)]
    saved_console_mode: Option<SavedConsoleMode>,
    in_raw_mode: bool,
    in_alternate_screen: bool,
    cursor_hidden: bool,
    mouse_enabled: bool,
    bracketed_paste_enabled: bool,
}

impl<W: Write> Terminal<W> {
    pub fn new(writer: W) -> io::Result<Self> {
        let size = Size::from_terminal()
            .unwrap_or_else(|_| Size::from_env().unwrap_or_else(Size::default_size));

        Ok(Terminal {
            writer,
            size,
            #[cfg(unix)]
            saved_termios: None,
            #[cfg(windows)]
            saved_console_mode: None,
            in_raw_mode: false,
            in_alternate_screen: false,
            cursor_hidden: false,
            mouse_enabled: false,
            bracketed_paste_enabled: false,
        })
    }

    /// Update the cached terminal size.
    pub fn update_size(&mut self) {
        self.size = Size::from_terminal()
            .unwrap_or_else(|_| Size::from_env().unwrap_or_else(Size::default_size));
    }

    /// Override the cached terminal size.
    ///
    /// In production, the size is discovered via `ioctl`/`COLUMNS`/`LINES`.
    /// Tests construct a `Terminal` with a writer that has no real size
    /// (e.g. `TestWriter`) or with a `TestBackend` whose size is ignored by
    /// `new()`. This lets them pin a deterministic size so layout-dependent
    /// assertions don't flake based on the host terminal.
    pub fn set_size(&mut self, size: Size) {
        self.size = size;
    }

    /// Get the current terminal size.
    pub fn size(&self) -> Size {
        self.size
    }

    // ── Raw mode ────────────────────────────────────────────────────────

    /// Switch the terminal to raw mode (no echo, no line buffering,
    /// no signal processing, character-at-a-time input).
    #[cfg(unix)]
    pub fn enter_raw_mode(&mut self) -> io::Result<()> {
        if self.in_raw_mode {
            return Ok(());
        }

        // Get current terminal attributes
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        let result = unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut termios) };
        if result == -1 {
            return Err(io::Error::last_os_error());
        }

        // Save original settings
        self.saved_termios = Some(SavedTermios { original: termios });

        // Modify for raw mode
        // Turn off: ECHO (echo), ICANON (line buffering), ISIG (signals),
        // IEXTEN (extended processing), OPOST (output processing)
        let new_termios = {
            let mut t = termios;
            t.c_iflag &= !(libc::BRKINT | libc::ICRNL | libc::INPCK | libc::ISTRIP | libc::IXON);
            t.c_oflag &= !libc::OPOST;
            t.c_cflag |= libc::CS8;
            t.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN);
            // Minimum bytes for read: 1 (character-at-a-time)
            t.c_cc[libc::VMIN] = 1;
            // Timeout: 0 (blocking read, no timeout)
            t.c_cc[libc::VTIME] = 0;
            t
        };

        // Apply new settings (drain any pending output first)
        let result = unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &new_termios) };
        if result == -1 {
            return Err(io::Error::last_os_error());
        }

        self.in_raw_mode = true;
        Ok(())
    }

    /// Restore the terminal to its original settings (Unix).
    #[cfg(unix)]
    pub fn leave_raw_mode(&mut self) -> io::Result<()> {
        if !self.in_raw_mode {
            return Ok(());
        }

        if let Some(saved) = &self.saved_termios {
            let result =
                unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSAFLUSH, &saved.original) };
            if result == -1 {
                return Err(io::Error::last_os_error());
            }
        }

        self.in_raw_mode = false;
        Ok(())
    }

    /// Switch the terminal to raw mode on Windows.
    ///
    /// Disables line input and echo on the console input handle, and
    /// enables virtual terminal processing on the output handle so that
    /// ANSI escape sequences are interpreted by the console.
    #[cfg(windows)]
    pub fn enter_raw_mode(&mut self) -> io::Result<()> {
        if self.in_raw_mode {
            return Ok(());
        }

        use self::winapi::*;

        let stdin = stdin_handle();
        if stdin.is_null() {
            return Err(io::Error::new(io::ErrorKind::Other, "GetStdHandle failed"));
        }

        // Save original stdin mode
        let original_mode = winapi::get_console_mode(stdin)?;
        self.saved_console_mode = Some(SavedConsoleMode {
            stdin_mode: original_mode,
        });

        // Disable echo and line input; enable virtual terminal input
        let new_mode =
            original_mode & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT);
        let new_mode = new_mode | ENABLE_VIRTUAL_TERMINAL_INPUT;
        winapi::set_console_mode(stdin, new_mode)?;

        // Also enable VT processing on stdout so ANSI escape sequences work
        let stdout = stdout_handle();
        if !stdout.is_null() {
            if let Ok(out_mode) = winapi::get_console_mode(stdout) {
                let _ =
                    winapi::set_console_mode(stdout, out_mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING);
            }
        }

        self.in_raw_mode = true;
        Ok(())
    }

    /// Restore the terminal to its original settings (Windows).
    #[cfg(windows)]
    pub fn leave_raw_mode(&mut self) -> io::Result<()> {
        if !self.in_raw_mode {
            return Ok(());
        }

        use self::winapi::*;

        if let Some(saved) = &self.saved_console_mode {
            let stdin = stdin_handle();
            if !stdin.is_null() {
                let _ = winapi::set_console_mode(stdin, saved.stdin_mode);
            }
        }

        self.in_raw_mode = false;
        Ok(())
    }

    /// Raw mode is not supported on other platforms.
    #[cfg(not(any(unix, windows)))]
    pub fn enter_raw_mode(&mut self) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "raw mode not supported on this platform",
        ))
    }

    /// No-op restore on platforms without raw mode support.
    #[cfg(not(any(unix, windows)))]
    pub fn leave_raw_mode(&mut self) -> io::Result<()> {
        Ok(())
    }

    // ── Alternate screen buffer ─────────────────────────────────────────

    /// Switch to the alternate screen buffer.
    ///
    /// This preserves the original terminal content. Call
    /// `leave_alternate_screen()` to restore it.
    pub fn enter_alternate_screen(&mut self) -> io::Result<()> {
        if self.in_alternate_screen {
            return Ok(());
        }
        self.writer.write_all(b"\x1b[?1049h")?;
        self.writer.flush()?;
        self.in_alternate_screen = true;
        Ok(())
    }

    /// Switch back to the main screen buffer.
    pub fn leave_alternate_screen(&mut self) -> io::Result<()> {
        if !self.in_alternate_screen {
            return Ok(());
        }
        self.writer.write_all(b"\x1b[?1049l")?;
        self.writer.flush()?;
        self.in_alternate_screen = false;
        Ok(())
    }

    // ── Cursor control ─────────────────────────────────────────────────

    /// Move the cursor to the specified row and column (1-based).
    pub fn set_cursor_pos(&mut self, row: u16, col: u16) -> io::Result<()> {
        write!(self.writer, "\x1b[{};{}H", row, col)?;
        Ok(())
    }

    /// Hide the cursor.
    pub fn hide_cursor(&mut self) -> io::Result<()> {
        if self.cursor_hidden {
            return Ok(());
        }
        self.writer.write_all(b"\x1b[?25l")?;
        self.writer.flush()?;
        self.cursor_hidden = true;
        Ok(())
    }

    /// Show the cursor.
    pub fn show_cursor(&mut self) -> io::Result<()> {
        if !self.cursor_hidden {
            return Ok(());
        }
        self.writer.write_all(b"\x1b[?25h")?;
        self.writer.flush()?;
        self.cursor_hidden = false;
        Ok(())
    }

    // ── Mouse tracking ──────────────────────────────────────────────────

    /// Enable mouse tracking (button presses, releases, and scrolling).
    pub fn enable_mouse(&mut self) -> io::Result<()> {
        if self.mouse_enabled {
            return Ok(());
        }
        // Enable basic mouse tracking (press/release)
        self.writer.write_all(b"\x1b[?1000h")?;
        // Enable button-motion tracking (drag)
        self.writer.write_all(b"\x1b[?1002h")?;
        // Enable SGR mouse mode (better coordinate reporting)
        self.writer.write_all(b"\x1b[?1006h")?;
        self.writer.flush()?;
        self.mouse_enabled = true;
        Ok(())
    }

    /// Disable mouse tracking.
    pub fn disable_mouse(&mut self) -> io::Result<()> {
        if !self.mouse_enabled {
            return Ok(());
        }
        // Disable in reverse order
        self.writer.write_all(b"\x1b[?1006l")?;
        self.writer.write_all(b"\x1b[?1002l")?;
        self.writer.write_all(b"\x1b[?1000l")?;
        self.writer.flush()?;
        self.mouse_enabled = false;
        Ok(())
    }

    // ── Bracketed paste ─────────────────────────────────────────────────

    /// Enable bracketed paste mode.
    ///
    /// When enabled, pasted text is surrounded by escape sequences:
    /// `\x1b[200~` (paste start) and `\x1b[201~` (paste end).
    /// This allows the TUI to treat pasted text as a single input event
    /// rather than individual key presses.
    pub fn enable_bracketed_paste(&mut self) -> io::Result<()> {
        if self.bracketed_paste_enabled {
            return Ok(());
        }
        self.writer.write_all(b"\x1b[?2004h")?;
        self.writer.flush()?;
        self.bracketed_paste_enabled = true;
        Ok(())
    }

    /// Disable bracketed paste mode.
    pub fn disable_bracketed_paste(&mut self) -> io::Result<()> {
        if !self.bracketed_paste_enabled {
            return Ok(());
        }
        self.writer.write_all(b"\x1b[?2004l")?;
        self.writer.flush()?;
        self.bracketed_paste_enabled = false;
        Ok(())
    }

    // ── Screen control ─────────────────────────────────────────────────

    /// Clear the entire screen and move cursor to (1, 1).
    pub fn clear_screen(&mut self) -> io::Result<()> {
        self.writer.write_all(b"\x1b[2J\x1b[H")?;
        self.writer.flush()
    }

    /// Clear from cursor to end of line.
    pub fn clear_to_eol(&mut self) -> io::Result<()> {
        self.writer.write_all(b"\x1b[K")?;
        Ok(())
    }

    /// Write raw bytes to the terminal.
    pub fn write_raw(&mut self, data: &[u8]) -> io::Result<()> {
        self.writer.write_all(data)
    }

    /// Flush buffered output.
    pub fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl<W: Write> Write for Terminal<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

impl<W: Write> Drop for Terminal<W> {
    fn drop(&mut self) {
        // Restore terminal state on drop
        let _ = self.disable_mouse();
        let _ = self.disable_bracketed_paste();
        let _ = self.show_cursor();
        let _ = self.leave_alternate_screen();
        let _ = self.leave_raw_mode();
    }
}

// ── Test backend (in-memory terminal) ────────────────────────────────────────

#[cfg(test)]
pub struct TestBackend {
    pub buffer: Vec<u8>,
    pub size: Size,
}

#[cfg(test)]
impl TestBackend {
    pub fn new(size: Size) -> Self {
        Self {
            buffer: Vec::new(),
            size,
        }
    }
}

#[cfg(test)]
impl std::io::Write for TestBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_size_default() {
        let s = Size::default_size();
        assert_eq!(s.cols, 80);
        assert_eq!(s.rows, 24);
    }

    #[test]
    fn test_size_new() {
        let s = Size::new(120, 40);
        assert_eq!(s.cols, 120);
        assert_eq!(s.rows, 40);
    }

    use std::io::Write;

    /// A writer that captures output into a `Vec<u8>` for testing.
    /// Unlike `Vec<u8>`, this doesn't have Drop-related borrow issues.
    struct TestWriter {
        buf: Vec<u8>,
    }

    impl TestWriter {
        fn new() -> Self {
            Self { buf: Vec::new() }
        }
    }

    impl Write for TestWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf.extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Helper: write some terminal commands, drop, and return the captured bytes.
    /// The Drop impl writes cleanup sequences, so we check that our sequences
    /// appear *anywhere* in the output, not just at the end.
    fn with_terminal<F: FnOnce(&mut Terminal<&mut TestWriter>)>(f: F) -> Vec<u8> {
        let mut writer = TestWriter::new();
        {
            let mut term = Terminal::new(&mut writer).unwrap();
            f(&mut term);
            // Drop restores terminal state, writing cleanup sequences
        }
        writer.buf
    }

    /// Check if a byte slice contains a specific subsequence.
    fn contains_seq(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn test_terminal_enter_leave_alternate_screen() {
        let buf = with_terminal(|term| {
            term.enter_alternate_screen().unwrap();
        });
        assert!(contains_seq(&buf, b"\x1b[?1049h"));
    }

    #[test]
    fn test_terminal_hide_show_cursor() {
        let buf = with_terminal(|term| {
            term.hide_cursor().unwrap();
        });
        assert!(contains_seq(&buf, b"\x1b[?25l"));

        let buf2 = with_terminal(|term| {
            term.hide_cursor().unwrap();
            term.show_cursor().unwrap();
        });
        assert!(contains_seq(&buf2, b"\x1b[?25h"));
    }

    #[test]
    fn test_terminal_enable_disable_mouse() {
        let buf = with_terminal(|term| {
            term.enable_mouse().unwrap();
        });
        assert!(contains_seq(&buf, b"\x1b[?1006h"));

        let buf2 = with_terminal(|term| {
            term.enable_mouse().unwrap();
            term.disable_mouse().unwrap();
        });
        assert!(contains_seq(&buf2, b"\x1b[?1000l"));
    }

    #[test]
    fn test_terminal_clear_screen() {
        let buf = with_terminal(|term| {
            term.clear_screen().unwrap();
        });
        assert!(contains_seq(&buf, b"\x1b[2J\x1b[H"));
    }

    #[test]
    fn test_terminal_set_cursor_pos() {
        let buf = with_terminal(|term| {
            term.set_cursor_pos(5, 10).unwrap();
        });
        assert!(contains_seq(&buf, b"\x1b[5;10H"));
    }

    #[test]
    fn test_terminal_bracketed_paste() {
        let buf = with_terminal(|term| {
            term.enable_bracketed_paste().unwrap();
        });
        assert!(contains_seq(&buf, b"\x1b[?2004h"));

        let buf2 = with_terminal(|term| {
            term.enable_bracketed_paste().unwrap();
            term.disable_bracketed_paste().unwrap();
        });
        assert!(contains_seq(&buf2, b"\x1b[?2004l"));
    }

    #[test]
    fn test_terminal_write_raw() {
        let buf = with_terminal(|term| {
            term.write_raw(b"hello").unwrap();
        });
        assert!(contains_seq(&buf, b"hello"));
    }
}
