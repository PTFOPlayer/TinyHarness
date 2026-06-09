// ── Terminal backend abstraction ──────────────────────────────────────────────
//
// All terminal writes go through a `Backend` trait so we can swap real
// terminal I/O for a test double. This makes the entire TUI testable
// without needing a real terminal.

use std::io::{self, Write};

use super::terminal::Size;

// ── Backend trait ────────────────────────────────────────────────────────────

/// Abstraction over terminal I/O.
///
/// The TUI writes all output through a `Backend` implementation. In
/// production, `StdioBackend` writes to stdout. In tests, `TestBackend`
/// captures output in memory.
pub trait Backend: Write {
    /// Get the current terminal size (columns, rows).
    fn size(&self) -> Size;

    /// Flush any buffered output.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// ── Stdio backend ────────────────────────────────────────────────────────────

/// Production backend that writes to stdout.
pub struct StdioBackend {
    stdout: io::Stdout,
}

impl StdioBackend {
    /// Create a new stdio backend.
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            stdout: io::stdout(),
        })
    }
}

impl Default for StdioBackend {
    fn default() -> Self {
        Self::new().expect("failed to create StdioBackend")
    }
}

impl Write for StdioBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stdout.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.stdout.flush()
    }
}

impl Backend for StdioBackend {
    fn size(&self) -> Size {
        Size::from_terminal()
            .unwrap_or_else(|_| Size::from_env().unwrap_or_else(Size::default_size))
    }
}

// ── Test backend ─────────────────────────────────────────────────────────────

/// In-memory backend for testing. Captures all output in a buffer.
pub struct TestBackend {
    buffer: Vec<u8>,
    size: Size,
}

impl TestBackend {
    /// Create a new test backend with the given terminal size.
    pub fn new(size: Size) -> Self {
        Self {
            buffer: Vec::new(),
            size,
        }
    }

    /// Get a reference to the captured output buffer.
    pub fn buffer(&self) -> &[u8] {
        &self.buffer
    }

    /// Take ownership of the captured output buffer, clearing it.
    pub fn take_buffer(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer)
    }

    /// Check if the captured output contains a specific byte sequence.
    pub fn contains(&self, needle: &[u8]) -> bool {
        self.buffer.windows(needle.len()).any(|w| w == needle)
    }
}

impl Write for TestBackend {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Backend for TestBackend {
    fn size(&self) -> Size {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_test_backend_write() {
        let mut backend = TestBackend::new(Size::new(80, 24));
        backend.write_all(b"hello").unwrap();
        assert!(backend.contains(b"hello"));
    }

    #[test]
    fn test_test_backend_take_buffer() {
        let mut backend = TestBackend::new(Size::new(80, 24));
        backend.write_all(b"hello").unwrap();
        let buf = backend.take_buffer();
        assert_eq!(&buf, b"hello");
        assert!(backend.buffer().is_empty());
    }

    #[test]
    fn test_test_backend_size() {
        let backend = TestBackend::new(Size::new(120, 40));
        let size = backend.size();
        assert_eq!(size.cols, 120);
        assert_eq!(size.rows, 40);
    }

    #[test]
    fn test_test_backend_contains() {
        let mut backend = TestBackend::new(Size::new(80, 24));
        backend.write_all(b"\x1b[?1049h\x1b[2J").unwrap();
        assert!(backend.contains(b"\x1b[?1049h"));
        assert!(backend.contains(b"\x1b[2J"));
        assert!(!backend.contains(b"\x1b[?25l"));
    }

    #[test]
    fn test_stdio_backend_new() {
        // Just verify it can be created
        let backend = StdioBackend::new();
        assert!(backend.is_ok());
    }

    #[test]
    fn test_stdio_backend_size() {
        let backend = StdioBackend::new().unwrap();
        let size = backend.size();
        // Should return something reasonable (at least 1x1)
        assert!(size.cols > 0);
        assert!(size.rows > 0);
    }
}
