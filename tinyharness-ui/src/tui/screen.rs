// ── Double-buffered screen ───────────────────────────────────────────────────
//
// The screen is a 2D grid of cells. Each frame, we compute a new grid and
// diff it against the previous frame. Only changed cells are written to
// the terminal, achieving flicker-free rendering.

use std::fmt;

use unicode_width::UnicodeWidthChar;

use super::cell::{Cell, Color, Style};
use super::layout::Rect;

// ── Wrap configuration ────────────────────────────────────────────────────────

/// Configuration for wrapped text rendering.
///
/// Controls wrapping, clipping, and skipping behavior for the
/// [`Screen::write_wrapped`] method.
struct WrapConfig {
    /// Maximum column number; text wraps when `col + char_width > wrap_col`.
    wrap_col: u16,
    /// Column where wrapped lines start (left margin for continuation lines).
    left_margin: u16,
    /// Maximum screen row; text stops when `screen_row > max_row`.
    max_row: u16,
    /// Number of visual rows to skip before rendering (for scroll offset).
    skip_rows: usize,
    /// If true, don't wrap — truncate at `wrap_col` instead.
    no_wrap: bool,
    /// If true, use screen-bounded wrapping (old `write_str_wrapped` semantics):
    /// wraps at screen width and clips at screen height.
    screen_bounded: bool,
}

// ── Screen ──────────────────────────────────────────────────────────────────

/// A double-buffered screen of cells.
///
/// The screen tracks the current state of every cell. When rendering,
/// the diff from the previous frame determines which cells need updating.
/// This avoids redrawing the entire screen on every frame.
pub struct Screen {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
}

impl Screen {
    /// Create a new screen with the given dimensions, filled with default cells.
    pub fn new(width: u16, height: u16) -> Self {
        let cells = vec![Cell::default(); (width as usize) * (height as usize)];
        Screen {
            width,
            height,
            cells,
        }
    }

    /// Resize the screen, clearing all content.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.cells = vec![Cell::default(); (width as usize) * (height as usize)];
    }

    /// Clear the entire screen to default cells.
    pub fn clear(&mut self) {
        self.cells.fill(Cell::default());
    }

    /// Get the screen width in columns.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Get the screen height in rows.
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Get a cell at the given position. Returns `None` if out of bounds.
    pub fn get(&self, row: u16, col: u16) -> Option<&Cell> {
        if row >= self.height || col >= self.width {
            return None;
        }
        self.cells
            .get((row as usize) * (self.width as usize) + (col as usize))
    }

    /// Get a mutable cell at the given position. Returns `None` if out of bounds.
    pub fn get_mut(&mut self, row: u16, col: u16) -> Option<&mut Cell> {
        if row >= self.height || col >= self.width {
            return None;
        }
        let idx = (row as usize) * (self.width as usize) + (col as usize);
        self.cells.get_mut(idx)
    }

    /// Set a cell at the given position. Does nothing if out of bounds.
    pub fn set_cell(&mut self, row: u16, col: u16, cell: Cell) {
        if let Some(c) = self.get_mut(row, col) {
            *c = cell;
        }
    }

    /// Merge a zero-width combining mark into the previous cell.
    ///
    /// Does nothing if `col` is at the start of the current rendering run
    /// or if `in_view` is false. If the preceding cell is a wide-character
    /// continuation cell, the mark is merged into the wide character's
    /// primary cell (two columns back) instead, since continuation cells
    /// are skipped during rendering.
    fn merge_combining_mark(
        &mut self,
        row: u16,
        col: u16,
        start_col: u16,
        ch: char,
        fg: Color,
        bg: Color,
        style: Style,
        in_view: bool,
    ) {
        if !in_view || col <= start_col {
            return;
        }
        // If the immediately preceding cell is a wide continuation cell,
        // the actual character is one column further back.
        let target_col = if let Some(prev) = self.get(row, col - 1) {
            if prev.wide && col >= 2 {
                col - 2
            } else {
                col - 1
            }
        } else {
            col - 1
        };
        if target_col < start_col {
            return;
        }
        if let Some(prev) = self.get_mut(row, target_col) {
            prev.char = ch;
            prev.fg = fg;
            prev.bg = bg;
            prev.style = style;
        }
    }

    /// Write a string starting at the given position, with the given style.
    ///
    /// Characters that exceed the screen width are truncated. Each character
    /// is placed according to its Unicode display width; zero-width chars
    /// (e.g. combining marks) overwrite the previous cell. Wide (CJK/
    /// fullwidth) characters that occupy 2 columns get a continuation
    /// cell marked at `col+1` so the renderer can skip it.
    pub fn write_str(
        &mut self,
        row: u16,
        col: u16,
        text: &str,
        fg: Color,
        bg: Color,
        style: Style,
    ) {
        let mut c = col;
        for ch in text.chars() {
            if c >= self.width {
                break;
            }
            let width = ch.width().unwrap_or(1);
            if width == 0 {
                self.merge_combining_mark(row, c, col, ch, fg, bg, style, c < self.width);
                continue;
            }
            // Don't write a wide character if it doesn't fully fit — the
            // terminal would render it as 2 columns but only 1 cell would
            // be tracked, causing cursor misalignment in render_diff.
            if width > 1 && c + 1 >= self.width {
                break;
            }
            self.set_cell(
                row,
                c,
                Cell {
                    char: ch,
                    fg,
                    bg,
                    style,
                    wide: false,
                },
            );
            if width > 1 && c + 1 < self.width {
                self.set_cell(row, c + 1, Cell::wide_continuation(fg, bg, style));
            }
            c += width as u16;
        }
    }

    /// Write a string starting at the given position, truncating or wrapping.
    ///
    /// If `wrap` is true, text wraps to the next line. If false, text is
    /// truncated at the right edge. Uses Unicode display widths.
    ///
    /// This is a convenience wrapper around `Self::write_wrapped` with
    /// simple wrapping bounded by the screen dimensions.
    pub fn write_str_wrapped(
        &mut self,
        start_row: u16,
        start_col: u16,
        text: &str,
        fg: Color,
        bg: Color,
        style: Style,
        wrap: bool,
    ) -> u16 {
        self.write_wrapped(
            start_row,
            start_col,
            text,
            fg,
            bg,
            style,
            WrapConfig {
                wrap_col: self.width,
                left_margin: 0,
                max_row: self.height.saturating_sub(1),
                skip_rows: 0,
                no_wrap: !wrap,
                screen_bounded: true,
            },
        )
    }

    /// Write a string with wrapping, but clip rendering at the given maximum row
    /// and wrap at the given column.
    ///
    /// `wrap_col` is the maximum column number; text wraps when `col >= wrap_col`.
    /// `max_row` is the maximum row; text stops when `row > max_row`.
    /// `left_margin` is the column where wrapped lines start. Uses Unicode display widths.
    ///
    /// This is a convenience wrapper around `Self::write_wrapped` with
    /// `skip_rows = 0` and wrapping enabled.
    pub fn write_str_wrapped_clipped(
        &mut self,
        start_row: u16,
        start_col: u16,
        text: &str,
        fg: Color,
        bg: Color,
        style: Style,
        left_margin: u16,
        max_row: u16,
        wrap_col: u16,
    ) -> u16 {
        self.write_wrapped(
            start_row,
            start_col,
            text,
            fg,
            bg,
            style,
            WrapConfig {
                wrap_col,
                left_margin,
                max_row,
                skip_rows: 0,
                no_wrap: false,
                screen_bounded: false,
            },
        )
    }

    /// Write a string with wrapping, skip the first `skip_rows` visual rows,
    /// and clip rendering at the given maximum row and wrap column.
    ///
    /// `wrap_col` is the maximum column number; text wraps when `col >= wrap_col`.
    /// `skip_rows` is the number of visual rows to skip before rendering.
    /// `max_row` is the maximum row; text stops when `row > max_row`.
    /// `left_margin` is the column where wrapped lines start. Uses Unicode display widths.
    ///
    /// This is a convenience wrapper around `Self::write_wrapped` with
    /// `skip_rows > 0`.
    pub fn write_str_wrapped_skip_clipped(
        &mut self,
        start_row: u16,
        start_col: u16,
        text: &str,
        fg: Color,
        bg: Color,
        style: Style,
        left_margin: u16,
        max_row: u16,
        wrap_col: u16,
        skip_rows: usize,
    ) {
        self.write_wrapped(
            start_row,
            start_col,
            text,
            fg,
            bg,
            style,
            WrapConfig {
                wrap_col,
                left_margin,
                max_row,
                skip_rows,
                no_wrap: false,
                screen_bounded: false,
            },
        );
    }

    /// Unified wrapped text writing method.
    ///
    /// Handles all wrapping scenarios through a single [`WrapConfig`] struct.
    /// The method tracks both a visual row counter (for skip/clipping) and a
    /// screen row counter (for actual cell placement).
    ///
    /// Returns the final screen row where text ended.
    fn write_wrapped(
        &mut self,
        start_row: u16,
        start_col: u16,
        text: &str,
        fg: Color,
        bg: Color,
        style: Style,
        cfg: WrapConfig,
    ) -> u16 {
        let mut visual_row: usize = 0;
        let mut col = start_col;
        let mut screen_row = start_row;

        for ch in text.chars() {
            let width = ch.width().unwrap_or(1);
            if width == 0 {
                let in_view = if cfg.skip_rows > 0 {
                    visual_row >= cfg.skip_rows && screen_row <= cfg.max_row
                } else if cfg.screen_bounded {
                    col < self.width && screen_row < self.height
                } else {
                    screen_row <= cfg.max_row
                };
                self.merge_combining_mark(screen_row, col, start_col, ch, fg, bg, style, in_view);
                continue;
            }
            let width_u16 = width as u16;

            // Handle newline
            if ch == '\n' {
                visual_row += 1;
                col = cfg.left_margin;
                if cfg.skip_rows == 0 || visual_row > cfg.skip_rows {
                    screen_row += 1;
                }
                if screen_row > cfg.max_row {
                    break;
                }
                continue;
            }

            // Handle wrap
            if col + width_u16 > cfg.wrap_col {
                if cfg.no_wrap {
                    break;
                }
                visual_row += 1;
                col = cfg.left_margin;
                if cfg.skip_rows == 0 || visual_row > cfg.skip_rows {
                    screen_row += 1;
                }
                if screen_row > cfg.max_row {
                    break;
                }
                // For screen_bounded mode (old write_str_wrapped), check screen height
                if cfg.screen_bounded && screen_row >= self.height {
                    break;
                }
            }

            // Only write the cell if we're past the skip zone and within bounds
            let past_skip = cfg.skip_rows == 0 || visual_row >= cfg.skip_rows;
            if past_skip && screen_row <= cfg.max_row {
                // Don't write a wide character if it doesn't fully fit on the
                // screen — the terminal renders it as 2 columns but only 1
                // cell would be tracked, causing cursor misalignment.
                if width > 1 && col + 1 >= self.width {
                    break;
                }
                self.set_cell(
                    screen_row,
                    col,
                    Cell {
                        char: ch,
                        fg,
                        bg,
                        style,
                        wide: false,
                    },
                );
                if width > 1 && col + 1 < self.width {
                    self.set_cell(screen_row, col + 1, Cell::wide_continuation(fg, bg, style));
                }
            }

            col += width_u16;
        }

        screen_row
    }

    /// Fill a rectangular area with the given cell.
    pub fn fill_rect(&mut self, rect: Rect, cell: Cell) {
        for row in rect.y..rect.y + rect.height {
            for col in rect.x..rect.x + rect.width {
                if row < self.height && col < self.width {
                    self.set_cell(row, col, cell.clone());
                }
            }
        }
    }

    /// Draw a horizontal line using the given character.
    pub fn hline(
        &mut self,
        row: u16,
        col_start: u16,
        col_end: u16,
        ch: char,
        fg: Color,
        bg: Color,
    ) {
        for col in col_start..=col_end.min(self.width.saturating_sub(1)) {
            self.set_cell(
                row,
                col,
                Cell {
                    char: ch,
                    fg,
                    bg,
                    style: Style::default(),
                    wide: false,
                },
            );
        }
    }

    /// Draw a vertical line using the given character.
    pub fn vline(
        &mut self,
        col: u16,
        row_start: u16,
        row_end: u16,
        ch: char,
        fg: Color,
        bg: Color,
    ) {
        for row in row_start..=row_end.min(self.height.saturating_sub(1)) {
            self.set_cell(
                row,
                col,
                Cell {
                    char: ch,
                    fg,
                    bg,
                    style: Style::default(),
                    wide: false,
                },
            );
        }
    }

    /// Draw a box (border) around a rectangular area.
    pub fn draw_box(&mut self, rect: Rect, fg: Color, bg: Color, style: Style) {
        let x = rect.x;
        let y = rect.y;
        let w = rect.width;
        let h = rect.height;

        if w < 2 || h < 2 {
            return;
        }

        // Corners
        self.set_cell(
            y,
            x,
            Cell {
                char: '┌',
                fg,
                bg,
                style,
                wide: false,
            },
        );
        self.set_cell(
            y,
            x + w - 1,
            Cell {
                char: '┐',
                fg,
                bg,
                style,
                wide: false,
            },
        );
        self.set_cell(
            y + h - 1,
            x,
            Cell {
                char: '└',
                fg,
                bg,
                style,
                wide: false,
            },
        );
        self.set_cell(
            y + h - 1,
            x + w - 1,
            Cell {
                char: '┘',
                fg,
                bg,
                style,
                wide: false,
            },
        );

        // Top and bottom borders
        for col in (x + 1)..(x + w - 1) {
            self.set_cell(
                y,
                col,
                Cell {
                    char: '─',
                    fg,
                    bg,
                    style,
                    wide: false,
                },
            );
            self.set_cell(
                y + h - 1,
                col,
                Cell {
                    char: '─',
                    fg,
                    bg,
                    style,
                    wide: false,
                },
            );
        }

        // Left and right borders
        for row in (y + 1)..(y + h - 1) {
            self.set_cell(
                row,
                x,
                Cell {
                    char: '│',
                    fg,
                    bg,
                    style,
                    wide: false,
                },
            );
            self.set_cell(
                row,
                x + w - 1,
                Cell {
                    char: '│',
                    fg,
                    bg,
                    style,
                    wide: false,
                },
            );
        }
    }

    // ── Diff-based rendering ────────────────────────────────────────────

    /// Compute the diff between this screen and a previous frame.
    ///
    /// Returns a list of `DiffOp` entries that, when applied in order,
    /// will bring the terminal from the previous state to the current state.
    pub fn diff_from(&self, previous: &Screen) -> Vec<DiffOp> {
        let mut ops = Vec::new();
        let max_row = self.height.min(previous.height);
        let max_col = self.width.min(previous.width);

        for row in 0..max_row {
            for col in 0..max_col {
                let prev_cell = previous.get(row, col);
                let curr_cell = self.get(row, col);

                match (prev_cell, curr_cell) {
                    (Some(prev), Some(curr)) if prev != curr => {
                        ops.push(DiffOp::SetCell {
                            row,
                            col,
                            cell: curr.clone(),
                        });
                    }
                    (None, Some(curr)) => {
                        ops.push(DiffOp::SetCell {
                            row,
                            col,
                            cell: curr.clone(),
                        });
                    }
                    _ => {}
                }
            }
        }

        // Handle rows that exist in the new screen but not the old one
        for row in previous.height..self.height {
            for col in 0..self.width {
                if let Some(cell) = self.get(row, col) {
                    ops.push(DiffOp::SetCell {
                        row,
                        col,
                        cell: cell.clone(),
                    });
                }
            }
        }

        ops
    }

    /// Render a list of diff operations to an ANSI escape sequence string.
    ///
    /// This is the core of the efficient rendering: we only write cells
    /// that actually changed, and we batch cursor movements.
    ///
    /// Handles wide (CJK/fullwidth) characters correctly by skipping
    /// continuation cells and tracking display width for cursor position.
    ///
    /// Optimizations for terminal throughput:
    /// - Consecutive cells on the same row skip the cursor move (cursor
    ///   naturally advances after writing a character).
    /// - Style reset (`\x1b[0m`) + re-application is only emitted when the
    ///   style actually changes from the previous cell, not per-cell.
    /// - Cells with default style and colors (typically spaces) skip the
    ///   style/fg/bg escape sequences entirely.
    pub fn render_diff(ops: &[DiffOp], width: u16) -> String {
        use unicode_width::UnicodeWidthChar;

        if ops.is_empty() {
            return String::new();
        }

        let mut output = String::with_capacity(ops.len() * 24);
        let mut last_row: Option<u16> = None;
        let mut last_col: Option<u16> = None;
        // Track the currently active style on the terminal so we only emit
        // changes, not a full reset+reapply for every cell.
        let mut active_fg: Option<Color> = None;
        let mut active_bg: Option<Color> = None;
        let mut active_style: Option<Style> = None;

        for op in ops {
            match op {
                DiffOp::SetCell { row, col, cell } => {
                    // Skip continuation cells — they're rendered as part of
                    // the wide character in the preceding column
                    if cell.wide {
                        continue;
                    }

                    // Move cursor if needed
                    // last_col stores the position the cursor should be at after
                    // writing the previous character (col + char_width), so the
                    // next character is expected at exactly last_col — no +1 needed.
                    let need_move = last_row != Some(*row) || last_col != Some(*col);

                    if need_move {
                        output.push_str(&format!("\x1b[{};{}H", row + 1, col + 1));
                    }

                    // Apply style/fg/bg only when they differ from what's
                    // currently active on the terminal. This avoids flooding
                    // the terminal with redundant escape sequences.
                    let fg_changed = active_fg != Some(cell.fg);
                    let bg_changed = active_bg != Some(cell.bg);
                    let style_changed = active_style != Some(cell.style);
                    let needs_reset = style_changed
                        || (fg_changed && cell.fg != Color::Default)
                        || (bg_changed && cell.bg != Color::Default);

                    if needs_reset {
                        // Full reset + reapply is safest when style attributes
                        // (bold/dim/etc.) change, because you can't selectively
                        // unset bold without resetting everything.
                        output.push_str("\x1b[0m");
                        active_fg = None;
                        active_bg = None;
                        active_style = None;
                    }

                    // Apply style if changed (or was just reset)
                    if active_style != Some(cell.style) {
                        output.push_str(&cell.style.escape());
                        active_style = Some(cell.style);
                    }

                    // Apply fg if changed (or was just reset)
                    if active_fg != Some(cell.fg) {
                        output.push_str(&cell.fg.fg_escape());
                        active_fg = Some(cell.fg);
                    }

                    // Apply bg if changed (or was just reset)
                    if active_bg != Some(cell.bg) {
                        output.push_str(&cell.bg.bg_escape());
                        active_bg = Some(cell.bg);
                    }

                    // Write character
                    output.push(cell.char);

                    // Track cursor position accounting for display width
                    let char_width = cell.char.width().unwrap_or(1).max(1) as u16;
                    last_row = Some(*row);
                    last_col = Some(*col + char_width);

                    // If we're at the right edge, the cursor won't advance
                    // further, so we need to move it explicitly next time
                    if *col + char_width >= width {
                        last_col = None;
                    }
                }
            }
        }

        // Reset all styles at the end
        output.push_str(Style::reset());

        output
    }
}

impl Clone for Screen {
    fn clone(&self) -> Self {
        Screen {
            width: self.width,
            height: self.height,
            cells: self.cells.clone(),
        }
    }
}

impl fmt::Debug for Screen {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Screen")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}

// ── Diff operation ───────────────────────────────────────────────────────────

/// A single rendering operation produced by diffing two screens.
#[derive(Clone, Debug, PartialEq)]
pub enum DiffOp {
    /// Set the cell at (row, col) to the given value.
    SetCell { row: u16, col: u16, cell: Cell },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screen_new() {
        let s = Screen::new(10, 5);
        assert_eq!(s.width(), 10);
        assert_eq!(s.height(), 5);
        // All cells should be default (space)
        assert_eq!(s.get(0, 0).unwrap().char, ' ');
    }

    #[test]
    fn test_screen_set_get_cell() {
        let mut s = Screen::new(10, 5);
        let cell = Cell::styled('X', Color::RED, Color::Default, Style::bold());
        s.set_cell(2, 3, cell.clone());
        assert_eq!(s.get(2, 3).unwrap(), &cell);
    }

    #[test]
    fn test_screen_out_of_bounds() {
        let s = Screen::new(10, 5);
        assert!(s.get(5, 0).is_none());
        assert!(s.get(0, 10).is_none());
    }

    #[test]
    fn test_screen_write_str() {
        let mut s = Screen::new(20, 5);
        s.write_str(1, 2, "Hello", Color::GREEN, Color::Default, Style::new());
        assert_eq!(s.get(1, 2).unwrap().char, 'H');
        assert_eq!(s.get(1, 3).unwrap().char, 'e');
        assert_eq!(s.get(1, 6).unwrap().char, 'o');
        assert_eq!(s.get(1, 7).unwrap().char, ' '); // default
    }

    #[test]
    fn test_screen_write_str_truncates() {
        let mut s = Screen::new(5, 1);
        s.write_str(
            0,
            0,
            "Hello World",
            Color::Default,
            Color::Default,
            Style::new(),
        );
        assert_eq!(s.get(0, 4).unwrap().char, 'o'); // 5th char (index 4)
        // "World" should be truncated
    }

    #[test]
    fn test_screen_clear() {
        let mut s = Screen::new(10, 5);
        s.set_cell(0, 0, Cell::char('X'));
        s.clear();
        assert_eq!(s.get(0, 0).unwrap().char, ' ');
    }

    #[test]
    fn test_screen_resize() {
        let mut s = Screen::new(10, 5);
        s.set_cell(0, 0, Cell::char('X'));
        s.resize(20, 10);
        assert_eq!(s.width(), 20);
        assert_eq!(s.height(), 10);
        // Old content should be gone
        assert_eq!(s.get(0, 0).unwrap().char, ' ');
    }

    #[test]
    fn test_screen_draw_box() {
        let mut s = Screen::new(10, 5);
        s.draw_box(
            Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 5,
            },
            Color::BLUE,
            Color::Default,
            Style::default(),
        );
        assert_eq!(s.get(0, 0).unwrap().char, '┌');
        assert_eq!(s.get(0, 9).unwrap().char, '┐');
        assert_eq!(s.get(4, 0).unwrap().char, '└');
        assert_eq!(s.get(4, 9).unwrap().char, '┘');
        assert_eq!(s.get(0, 5).unwrap().char, '─');
        assert_eq!(s.get(2, 0).unwrap().char, '│');
    }

    #[test]
    fn test_screen_diff_no_changes() {
        let s1 = Screen::new(10, 5);
        let s2 = Screen::new(10, 5);
        let diff = s2.diff_from(&s1);
        assert!(diff.is_empty());
    }

    #[test]
    fn test_screen_diff_with_changes() {
        let s1 = Screen::new(10, 5);
        let mut s2 = Screen::new(10, 5);
        s2.set_cell(1, 2, Cell::char('X'));
        s2.set_cell(3, 4, Cell::char('Y'));

        let diff = s2.diff_from(&s1);
        assert_eq!(diff.len(), 2);
    }

    #[test]
    fn test_screen_render_diff() {
        let s1 = Screen::new(10, 5);
        let mut s2 = Screen::new(10, 5);
        s2.set_cell(
            0,
            0,
            Cell::styled('A', Color::RED, Color::Default, Style::bold()),
        );

        let diff = s2.diff_from(&s1);
        let rendered = Screen::render_diff(&diff, 10);

        // Should contain cursor movement and the character
        assert!(rendered.contains("\x1b[1;1H")); // move to (1,1)
        assert!(rendered.contains('A'));
        assert!(rendered.contains("\x1b[0m")); // reset at end
    }

    #[test]
    fn test_screen_hline() {
        let mut s = Screen::new(10, 5);
        s.hline(2, 1, 8, '─', Color::Default, Color::Default);
        assert_eq!(s.get(2, 1).unwrap().char, '─');
        assert_eq!(s.get(2, 8).unwrap().char, '─');
        assert_eq!(s.get(2, 0).unwrap().char, ' '); // before line
    }

    #[test]
    fn test_screen_vline() {
        let mut s = Screen::new(10, 5);
        s.vline(5, 1, 3, '│', Color::Default, Color::Default);
        assert_eq!(s.get(1, 5).unwrap().char, '│');
        assert_eq!(s.get(2, 5).unwrap().char, '│');
        assert_eq!(s.get(3, 5).unwrap().char, '│');
        assert_eq!(s.get(0, 5).unwrap().char, ' '); // before line
    }

    #[test]
    fn test_screen_fill_rect() {
        let mut s = Screen::new(10, 5);
        let rect = Rect {
            x: 2,
            y: 1,
            width: 3,
            height: 2,
        };
        s.fill_rect(rect, Cell::char('█'));

        assert_eq!(s.get(1, 2).unwrap().char, '█');
        assert_eq!(s.get(1, 4).unwrap().char, '█');
        assert_eq!(s.get(2, 2).unwrap().char, '█');
        assert_eq!(s.get(0, 2).unwrap().char, ' '); // outside rect
    }

    #[test]
    fn test_screen_write_str_wrapped() {
        let mut s = Screen::new(5, 5);
        let end_row = s.write_str_wrapped(
            0,
            0,
            "ABCDEFGH",
            Color::Default,
            Color::Default,
            Style::new(),
            true,
        );
        // "ABCDE" on row 0, "FGH" on row 1
        assert_eq!(s.get(0, 0).unwrap().char, 'A');
        assert_eq!(s.get(0, 4).unwrap().char, 'E');
        assert_eq!(s.get(1, 0).unwrap().char, 'F');
        assert_eq!(s.get(1, 2).unwrap().char, 'H');
        assert_eq!(end_row, 1);
    }

    #[test]
    fn test_screen_write_str_wrapped_skip_clipped() {
        let mut s = Screen::new(5, 5);
        // "ABCDE" on row 0, "FGH" on row 1
        // Skip the first row, render only "FGH" starting at screen row 0
        s.write_str_wrapped_skip_clipped(
            0,
            0,
            "ABCDEFGH",
            Color::Default,
            Color::Default,
            Style::new(),
            0,
            4,
            5, // wrap_col
            1, // skip 1 row
        );
        // Row 0 should have "FGH" (the 2nd visual row of the text)
        assert_eq!(s.get(0, 0).unwrap().char, 'F');
        assert_eq!(s.get(0, 2).unwrap().char, 'H');
        // Row 1 should be empty (default)
        assert_eq!(s.get(1, 0).unwrap().char, ' ');
    }

    #[test]
    fn test_screen_write_str_wrapped_skip_clipped_newlines() {
        let mut s = Screen::new(5, 5);
        // "AB\nCD" → row 0: "AB", row 1: "CD"
        // Skip 1 row, render "CD" starting at screen row 0
        s.write_str_wrapped_skip_clipped(
            0,
            0,
            "AB\nCD",
            Color::Default,
            Color::Default,
            Style::new(),
            0,
            4,
            5,
            1,
        );
        assert_eq!(s.get(0, 0).unwrap().char, 'C');
        assert_eq!(s.get(0, 1).unwrap().char, 'D');
    }

    #[test]
    fn test_screen_write_str_wide_char() {
        // Wide (CJK) characters should occupy 2 columns and mark continuation cell
        let mut s = Screen::new(10, 3);
        // '一' is a CJK character with display width 2
        s.write_str(0, 0, "一x", Color::Default, Color::Default, Style::new());

        // The wide char should be at col 0
        let cell_0 = s.get(0, 0).unwrap();
        assert_eq!(cell_0.char, '一');
        assert!(!cell_0.wide);

        // The continuation cell should be at col 1
        let cell_1 = s.get(0, 1).unwrap();
        assert!(cell_1.wide);

        // 'x' should be at col 2 (not col 1)
        let cell_2 = s.get(0, 2).unwrap();
        assert_eq!(cell_2.char, 'x');
        assert!(!cell_2.wide);
    }

    #[test]
    fn test_screen_write_str_wide_char_at_edge() {
        // Wide char at the right edge should not overflow
        let mut s = Screen::new(3, 1);
        s.write_str(0, 0, "一", Color::Default, Color::Default, Style::new());

        // '一' takes cols 0-1, which fits in width 3
        assert_eq!(s.get(0, 0).unwrap().char, '一');
        assert!(s.get(0, 1).unwrap().wide);
        assert_eq!(s.get(0, 2).unwrap().char, ' '); // empty
    }

    #[test]
    fn test_cell_default_not_wide() {
        let cell = Cell::default();
        assert!(!cell.wide);
        assert_eq!(cell.char, ' ');
    }

    #[test]
    fn test_cell_wide_continuation() {
        let cell = Cell::wide_continuation(Color::RED, Color::BLUE, Style::bold());
        assert!(cell.wide);
        assert_eq!(cell.char, ' ');
        assert_eq!(cell.fg, Color::RED);
        assert_eq!(cell.bg, Color::BLUE);
        assert!(cell.style.bold);
    }

    #[test]
    fn test_screen_diff_wide_char_tracking() {
        // When a wide char changes, the continuation cell should also be
        // included in the diff so it gets properly updated.
        let mut s1 = Screen::new(10, 1);
        let mut s2 = Screen::new(10, 1);
        s1.write_str(0, 0, "AB", Color::Default, Color::Default, Style::new());
        s2.write_str(0, 0, "一x", Color::Default, Color::Default, Style::new());

        let diff = s2.diff_from(&s1);

        // Should have diffs for: col 0 (一 replaces A), col 1 (continuation replaces B), col 2 (x replaces nothing)
        // At minimum, cols 0 and 1 must differ
        assert!(diff.len() >= 2);

        // Check that col 0 has the wide char
        let col0_op = diff
            .iter()
            .find(|op| matches!(op, DiffOp::SetCell { col: 0, .. }));
        assert!(col0_op.is_some());
        let DiffOp::SetCell { cell: cell0, .. } = col0_op.unwrap();
        assert_eq!(cell0.char, '一');
        assert!(!cell0.wide);

        // Check that col 1 has the continuation marker
        let col1_op = diff
            .iter()
            .find(|op| matches!(op, DiffOp::SetCell { col: 1, .. }));
        assert!(col1_op.is_some());
        let DiffOp::SetCell { cell: cell1, .. } = col1_op.unwrap();
        assert!(cell1.wide);
    }

    #[test]
    fn test_screen_write_str_wrapped_long_message_with_spaces() {
        // Simulate a long message with spaces, like what the conversation widget renders
        let mut s = Screen::new(10, 5);
        let _end_row = s.write_str_wrapped(
            0,
            0,
            "Hello World This Is A Test",
            Color::WHITE,
            Color::Default,
            Style::default(),
            true,
        );
        // With width 10:
        // Row 0: "Hello Worl" (10 chars, 'd' wraps)
        // Row 1: "d This Is " (10 chars, 'A' wraps)
        // Row 2: "A Test" (6 chars)
        // Verify wrapping preserves spaces correctly
        assert_eq!(s.get(0, 0).unwrap().char, 'H');
        assert_eq!(s.get(0, 5).unwrap().char, ' '); // space between "Hello" and "World"
        assert_eq!(s.get(1, 0).unwrap().char, 'd');
        assert_eq!(s.get(1, 1).unwrap().char, ' '); // space between "World" and "This"
        assert_eq!(s.get(2, 0).unwrap().char, 'A');
        assert_eq!(s.get(2, 1).unwrap().char, ' '); // space between "A" and "Test"
        assert_eq!(s.get(2, 2).unwrap().char, 'T');
    }

    #[test]
    fn test_screen_write_str_wrapped_clipped_multiline() {
        // Test clipped wrapping with a multi-line message
        let mut s = Screen::new(10, 5);
        let _end_row = s.write_str_wrapped_clipped(
            0,
            2,
            "AB CD",
            Color::WHITE,
            Color::Default,
            Style::default(),
            2,  // left_margin
            4,  // max_row
            10, // wrap_col
        );
        // "AB CD" starting at col 2, width 10
        // Row 0: "  AB CD" (fits within 8 cols from col 2)
        assert_eq!(s.get(0, 2).unwrap().char, 'A');
        assert_eq!(s.get(0, 3).unwrap().char, 'B');
        assert_eq!(s.get(0, 4).unwrap().char, ' ');
        assert_eq!(s.get(0, 5).unwrap().char, 'C');
        assert_eq!(s.get(0, 6).unwrap().char, 'D');
    }

    #[test]
    fn test_screen_write_str_wrapped_newlines() {
        // Test that newlines work correctly in wrapped mode
        let mut s = Screen::new(10, 5);
        s.write_str_wrapped(
            0,
            0,
            "AB\nCD",
            Color::WHITE,
            Color::Default,
            Style::default(),
            true,
        );
        // Row 0: "AB        "
        // Row 1: "CD        "
        assert_eq!(s.get(0, 0).unwrap().char, 'A');
        assert_eq!(s.get(0, 1).unwrap().char, 'B');
        assert_eq!(s.get(1, 0).unwrap().char, 'C');
        assert_eq!(s.get(1, 1).unwrap().char, 'D');
    }

    #[test]
    fn test_screen_write_str_wrapped_clipped_with_left_margin() {
        // Test wrapped clipping with left margin (like conversation messages)
        let mut s = Screen::new(20, 5);
        let _end_row = s.write_str_wrapped_clipped(
            0,
            7,
            "Hello World This Is A Long Message That Wraps",
            Color::WHITE,
            Color::Default,
            Style::default(),
            7,  // left_margin
            4,  // max_row
            20, // wrap_col
        );
        // First line starts at col 7, wraps at col 20
        // Subsequent lines start at col 7
        assert_eq!(s.get(0, 7).unwrap().char, 'H');
    }

    #[test]
    fn test_screen_clear_replaces_with_spaces() {
        // Verify that clear() properly resets cells to spaces
        let mut s = Screen::new(10, 3);
        s.write_str(
            0,
            0,
            "ABCDEFGHIJ",
            Color::WHITE,
            Color::Default,
            Style::default(),
        );
        assert_eq!(s.get(0, 0).unwrap().char, 'A');
        assert_eq!(s.get(0, 5).unwrap().char, 'F');
        s.clear();
        assert_eq!(s.get(0, 0).unwrap().char, ' ');
        assert_eq!(s.get(0, 5).unwrap().char, ' ');
    }

    #[test]
    fn test_screen_write_str_wrapped_skip_clipped_preserves_spaces() {
        // Test that spaces are preserved when using skip_clipped wrapping
        let mut s = Screen::new(20, 5);
        // "Hello World" with left_margin=2, starting at col 2
        // First row: "  Hello World" (fits in 18 cols)
        // Now skip 0 rows (render from start)
        s.write_str_wrapped_skip_clipped(
            0,
            2,
            "Hello World",
            Color::WHITE,
            Color::Default,
            Style::default(),
            2,  // left_margin
            4,  // max_row
            20, // wrap_col
            0,  // skip_rows
        );
        // The space between "Hello" and "World" should be preserved
        assert_eq!(s.get(0, 2).unwrap().char, 'H');
        assert_eq!(s.get(0, 7).unwrap().char, ' '); // space between Hello and World
        assert_eq!(s.get(0, 8).unwrap().char, 'W');
    }

    #[test]
    fn test_screen_write_str_wrapped_skip_clipped_multirow() {
        // Test a long message that wraps, with skip_rows
        let mut s = Screen::new(10, 5);
        // "ABCDEFGH" wraps to "ABCDEFGH" then "IJ"
        // With width 10: "ABCDEFGHIJ" fits on one row
        // Let's use something longer: "ABCDEFGHIJKLMNO"
        // Row 0: "ABCDEFGHIJ" (10 chars)
        // Row 1: "KLMNO" (5 chars)
        // Skip 1 row, render only row 1 at screen row 0
        s.write_str_wrapped_skip_clipped(
            0,
            0,
            "ABCDEFGHIJKLMNO",
            Color::WHITE,
            Color::Default,
            Style::default(),
            0,  // left_margin
            4,  // max_row
            10, // wrap_col
            1,  // skip 1 row (skip "ABCDEFGHIJ")
        );
        // Row 0 should have "KLMNO"
        assert_eq!(s.get(0, 0).unwrap().char, 'K');
        assert_eq!(s.get(0, 1).unwrap().char, 'L');
        assert_eq!(s.get(0, 4).unwrap().char, 'O');
    }

    #[test]
    fn test_screen_render_diff_dedupes_styles() {
        // Verify that render_diff doesn't emit redundant style/fg/bg
        // sequences for consecutive cells with the same style.
        let s1 = Screen::new(80, 24);
        let mut s2 = Screen::new(80, 24);
        // Write 10 consecutive characters with the SAME style
        s2.write_str(
            5,
            0,
            "ABCDEFGHIJ",
            Color::WHITE,
            Color::Default,
            Style::default(),
        );

        let diff = s2.diff_from(&s1);
        let rendered = Screen::render_diff(&diff, 80);

        // With the optimized render_diff, consecutive cells with the same
        // style should NOT repeat the fg/bg escape sequences.
        // For default style + WHITE fg + Default bg, we expect:
        // - 1 cursor move to start
        // - 1 reset + style + fg (since bg is default, no bg escape needed)
        // - 9 plain characters (no extra escapes)
        // - 1 reset at end
        let fg_count = rendered.matches("\x1b[37m").count(); // WHITE = Ansi(7) = \x1b[37m
        let reset_count = rendered.matches("\x1b[0m").count();

        // FG should only be set once (for the first cell), not 10 times
        assert!(
            fg_count <= 2,
            "Expected at most 2 fg sequences (one for first cell + one at final reset), got {}",
            fg_count
        );
        // Reset should be emitted at most once per style change + once at end
        assert!(
            reset_count <= 2,
            "Expected at most 2 resets (initial + final), got {}",
            reset_count
        );
    }

    #[test]
    fn test_screen_render_diff_preserves_correct_output() {
        // Verify that the optimized render_diff still produces correct output
        let s1 = Screen::new(80, 24);
        let mut s2 = Screen::new(80, 24);
        s2.write_str(
            5,
            0,
            "wrong chars",
            Color::WHITE,
            Color::Default,
            Style::default(),
        );

        let diff = s2.diff_from(&s1);
        let rendered = Screen::render_diff(&diff, 80);

        // The output should contain the actual characters in order
        // Extract just the printable characters from the output
        let printable: String = rendered
            .chars()
            .filter(|c| !c.is_control() && *c != '\x1b')
            .collect();
        // Should contain "wrong chars"
        assert!(
            printable.contains("wrong chars"),
            "Output should contain 'wrong chars', got printable: {:?}",
            printable
        );
    }

    #[test]
    fn test_screen_render_diff_style_change_forces_reset() {
        // When style changes (e.g., bold to dim), a reset must be emitted
        let s1 = Screen::new(80, 24);
        let mut s2 = Screen::new(80, 24);
        s2.write_str(0, 0, "AB", Color::WHITE, Color::Default, Style::bold());
        s2.write_str(0, 2, "CD", Color::WHITE, Color::Default, Style::dim());

        let diff = s2.diff_from(&s1);
        let rendered = Screen::render_diff(&diff, 80);

        // Should contain at least one reset (to switch from bold to dim)
        let reset_count = rendered.matches("\x1b[0m").count();
        assert!(
            reset_count >= 1,
            "Expected at least 1 reset for style change, got {}",
            reset_count
        );
    }

    #[test]
    fn test_screen_wide_char_does_not_overflow_last_column() {
        // A wide character at the last column should not be written as a
        // half-width glyph — it should be skipped entirely.
        let mut s = Screen::new(3, 1);
        // '一' has width 2. At col 2 (last column of a 3-wide screen),
        // it needs cols 2-3 but only col 2 exists.
        s.write_str(0, 2, "一", Color::Default, Color::Default, Style::new());
        // The cell at col 2 should remain default (space) — the wide char
        // was skipped because it didn't fully fit.
        assert_eq!(s.get(0, 2).unwrap().char, ' ');
        assert!(!s.get(0, 2).unwrap().wide);
    }

    #[test]
    fn test_screen_combining_mark_after_wide_char() {
        // A combining mark after a wide character should merge into the
        // wide character's primary cell, not the continuation cell.
        let mut s = Screen::new(10, 1);
        // '一' (width 2) + combining acute (U+0301, width 0)
        s.write_str(
            0,
            0,
            "一\u{0301}",
            Color::Default,
            Color::Default,
            Style::new(),
        );

        // The combining mark should be in col 0 (the wide char's primary cell),
        // not col 1 (the continuation cell).
        let cell_0 = s.get(0, 0).unwrap();
        assert_eq!(
            cell_0.char, '\u{0301}',
            "combining mark should be in primary cell"
        );
        assert!(!cell_0.wide);

        // Col 1 should still be the continuation cell
        let cell_1 = s.get(0, 1).unwrap();
        assert!(
            cell_1.wide,
            "col 1 should still be a wide continuation cell"
        );
    }

    #[test]
    fn test_screen_write_str_wide_char_at_last_col_breaks() {
        // "ab一" on a 3-wide screen: 'a' at 0, 'b' at 1, '一' at 2 needs 2-3.
        // Since col 3 doesn't exist, '一' should be skipped.
        let mut s = Screen::new(3, 1);
        s.write_str(0, 0, "ab一", Color::Default, Color::Default, Style::new());
        assert_eq!(s.get(0, 0).unwrap().char, 'a');
        assert_eq!(s.get(0, 1).unwrap().char, 'b');
        assert_eq!(s.get(0, 2).unwrap().char, ' '); // '一' was skipped
    }

    #[test]
    fn test_screen_write_wrapped_wide_char_at_last_col() {
        // In wrapped mode, a wide char at the last column should wrap to the
        // next line rather than being written as a half-width glyph.
        let mut s = Screen::new(3, 3);
        s.write_str_wrapped(
            0,
            0,
            "ab一c",
            Color::Default,
            Color::Default,
            Style::new(),
            true,
        );
        // Row 0: "ab" (cols 0-1), '一' doesn't fit at col 2, wraps to row 1
        assert_eq!(s.get(0, 0).unwrap().char, 'a');
        assert_eq!(s.get(0, 1).unwrap().char, 'b');
        assert_eq!(s.get(0, 2).unwrap().char, ' '); // empty (wide char wrapped)
        // Row 1: "一" (cols 0-1), then 'c' at col 2
        assert_eq!(s.get(1, 0).unwrap().char, '一');
        assert!(s.get(1, 1).unwrap().wide);
        assert_eq!(s.get(1, 2).unwrap().char, 'c');
    }
}
