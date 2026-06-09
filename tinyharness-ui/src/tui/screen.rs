// ── Double-buffered screen ───────────────────────────────────────────────────
//
// The screen is a 2D grid of cells. Each frame, we compute a new grid and
// diff it against the previous frame. Only changed cells are written to
// the terminal, achieving flicker-free rendering.

use std::fmt;

use super::cell::{Cell, Color, Style};
use super::layout::Rect;

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

    /// Write a string starting at the given position, with the given style.
    ///
    /// Characters that exceed the screen width are truncated.
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
            self.set_cell(
                row,
                c,
                Cell {
                    char: ch,
                    fg,
                    bg,
                    style,
                },
            );
            c += 1;
        }
    }

    /// Write a string starting at the given position, truncating or wrapping.
    ///
    /// If `wrap` is true, text wraps to the next line. If false, text is
    /// truncated at the right edge.
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
        let mut row = start_row;
        let mut col = start_col;

        for ch in text.chars() {
            if col >= self.width {
                if wrap && row + 1 < self.height {
                    row += 1;
                    col = 0;
                } else {
                    break;
                }
            }
            if ch == '\n' {
                row += 1;
                col = 0;
                continue;
            }
            self.set_cell(
                row,
                col,
                Cell {
                    char: ch,
                    fg,
                    bg,
                    style,
                },
            );
            col += 1;
        }

        row
    }

    /// Write a string with wrapping, but clip rendering at the given maximum row
    /// and wrap at the given column.
    ///
    /// `wrap_col` is the maximum column number; text wraps when `col >= wrap_col`.
    /// `max_row` is the maximum row; text stops when `row > max_row`.
    /// `left_margin` is the column where wrapped lines start.
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
        let mut row = start_row;
        let mut col = start_col;

        for ch in text.chars() {
            if col >= wrap_col {
                // Wrap to next line
                row += 1;
                col = left_margin;
            }
            // Stop if we've exceeded the max row
            if row > max_row {
                break;
            }
            if ch == '\n' {
                row += 1;
                col = left_margin;
                if row > max_row {
                    break;
                }
                continue;
            }
            self.set_cell(
                row,
                col,
                Cell {
                    char: ch,
                    fg,
                    bg,
                    style,
                },
            );
            col += 1;
        }

        row
    }

    /// Write a string with wrapping, skip the first `skip_rows` visual rows,
    /// and clip rendering at the given maximum row and wrap column.
    ///
    /// `wrap_col` is the maximum column number; text wraps when `col >= wrap_col`.
    /// `skip_rows` is the number of visual rows to skip before rendering.
    /// `max_row` is the maximum row; text stops when `row > max_row`.
    /// `left_margin` is the column where wrapped lines start.
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
        let mut visual_row: usize = 0;
        let mut col = start_col;
        let mut screen_row = start_row;

        for ch in text.chars() {
            // Check if we need to wrap before placing this character
            if ch != '\n' && col >= wrap_col {
                // Wrap to next visual line
                visual_row += 1;
                col = left_margin;
                // Only advance screen_row if we're past the renderable zone
                if visual_row > skip_rows {
                    screen_row += 1;
                }
                if screen_row > max_row {
                    break;
                }
            }

            if ch == '\n' {
                // Newline — advance to next visual row
                visual_row += 1;
                col = left_margin;
                if visual_row > skip_rows {
                    screen_row += 1;
                }
                if screen_row > max_row {
                    break;
                }
                continue;
            }

            // Only write the cell if we're past the skip zone
            if visual_row >= skip_rows && screen_row <= max_row {
                self.set_cell(
                    screen_row,
                    col,
                    Cell {
                        char: ch,
                        fg,
                        bg,
                        style,
                    },
                );
            }

            col += 1;
        }
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
    pub fn render_diff(ops: &[DiffOp], width: u16) -> String {
        if ops.is_empty() {
            return String::new();
        }

        let mut output = String::with_capacity(ops.len() * 20);
        let mut last_row: Option<u16> = None;
        let mut last_col: Option<u16> = None;

        for op in ops {
            match op {
                DiffOp::SetCell { row, col, cell } => {
                    // Move cursor if needed
                    let need_move = last_row != Some(*row) || last_col.unwrap_or(0) + 1 != *col;

                    if need_move {
                        output.push_str(&format!("\x1b[{};{}H", row + 1, col + 1));
                    }

                    // Apply style
                    output.push_str(&cell.style.escape());
                    // Apply foreground color
                    output.push_str(&cell.fg.fg_escape());
                    // Apply background color
                    output.push_str(&cell.bg.bg_escape());
                    // Write character
                    output.push(cell.char);

                    last_row = Some(*row);
                    last_col = Some(*col + 1);

                    // If we're at the right edge, the cursor won't advance
                    // further, so we need to move it explicitly next time
                    if *col + 1 >= width {
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
}
