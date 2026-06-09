// ── Constraint-based layout engine ──────────────────────────────────────────
//
// Splits rectangular areas into sub-areas based on constraints.
// Inspired by ratatui's layout system but implemented from scratch.

/// A rectangular area on the screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    /// The area (width * height) of this rectangle.
    pub fn area(&self) -> u32 {
        (self.width as u32) * (self.height as u32)
    }

    /// Returns true if this rectangle has zero area.
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Check if a point is inside this rectangle.
    pub fn contains(&self, x: u16, y: u16) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    /// Clamp this rectangle to fit within another rectangle.
    pub fn clamp_to(&self, other: Rect) -> Rect {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let max_x = (self.x + self.width).min(other.x + other.width);
        let max_y = (self.y + self.height).min(other.y + other.height);
        Rect {
            x,
            y,
            width: max_x.saturating_sub(x),
            height: max_y.saturating_sub(y),
        }
    }

    /// The right edge (x + width).
    pub fn right(&self) -> u16 {
        self.x + self.width
    }

    /// The bottom edge (y + height).
    pub fn bottom(&self) -> u16 {
        self.y + self.height
    }

    /// Shrink the rectangle by the given amount on all sides.
    pub fn shrink(&self, amount: u16) -> Rect {
        Rect {
            x: self.x + amount,
            y: self.y + amount,
            width: self.width.saturating_sub(amount * 2),
            height: self.height.saturating_sub(amount * 2),
        }
    }

    /// The inner area, shrunk by 1 cell on each side (for borders).
    pub fn inner(&self) -> Rect {
        self.shrink(1)
    }

    /// Split the rectangle horizontally (top/bottom) at the given row offset.
    pub fn split_horizontally(&self, at: u16) -> (Rect, Rect) {
        let top_height = at.min(self.height);
        let bottom_height = self.height.saturating_sub(top_height);
        (
            Rect::new(self.x, self.y, self.width, top_height),
            Rect::new(self.x, self.y + top_height, self.width, bottom_height),
        )
    }

    /// Split the rectangle vertically (left/right) at the given column offset.
    pub fn split_vertically(&self, at: u16) -> (Rect, Rect) {
        let left_width = at.min(self.width);
        let right_width = self.width.saturating_sub(left_width);
        (
            Rect::new(self.x, self.y, left_width, self.height),
            Rect::new(self.x + left_width, self.y, right_width, self.height),
        )
    }
}

/// Layout direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Stack areas horizontally (left to right).
    Horizontal,
    /// Stack areas vertically (top to bottom).
    Vertical,
}

/// A constraint for laying out areas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Constraint {
    /// Fixed size in cells.
    Length(u16),
    /// Percentage of the available space (0–100).
    Percentage(u16),
    /// At least this many cells.
    Min(u16),
    /// At most this many cells.
    Max(u16),
}

/// A layout specification.
///
/// Splits a rectangle into sub-rectangles based on constraints and direction.
pub struct Layout {
    pub direction: Direction,
    pub constraints: Vec<Constraint>,
    /// Gap between areas in cells (0 = no gap).
    pub gap: u16,
}

impl Layout {
    pub fn new(direction: Direction) -> Self {
        Self {
            direction,
            constraints: Vec::new(),
            gap: 0,
        }
    }

    pub fn constraints(mut self, constraints: Vec<Constraint>) -> Self {
        self.constraints = constraints;
        self
    }

    pub fn gap(mut self, gap: u16) -> Self {
        self.gap = gap;
        self
    }

    /// Split the given area into sub-rectangles based on the constraints.
    ///
    /// The algorithm:
    /// 1. Calculate the total available space (minus gaps).
    /// 2. Resolve each constraint into a desired size.
    /// 3. Distribute remaining space to `Min`/`Percentage` constraints.
    /// 4. Return the list of rectangles.
    pub fn split(self, area: Rect) -> Vec<Rect> {
        if self.constraints.is_empty() || area.is_empty() {
            return vec![];
        }

        let n = self.constraints.len();
        let total_gap = self.gap as u32 * n.saturating_sub(1) as u32;

        match self.direction {
            Direction::Vertical => self.split_vertical(area, total_gap),
            Direction::Horizontal => self.split_horizontal(area, total_gap),
        }
    }

    fn split_vertical(&self, area: Rect, total_gap: u32) -> Vec<Rect> {
        let available = area.height as u32;
        let available_after_gap = available.saturating_sub(total_gap);
        let sizes = self.resolve_sizes(available_after_gap, area.height);

        let mut rects = Vec::with_capacity(sizes.len());
        let mut y = area.y;
        for (i, &size) in sizes.iter().enumerate() {
            let height = size.min(area.y + area.height - y);
            if height == 0 {
                break;
            }
            rects.push(Rect::new(area.x, y, area.width, height));
            y += height;
            if i + 1 < sizes.len() {
                y += self.gap;
            }
        }
        rects
    }

    fn split_horizontal(&self, area: Rect, total_gap: u32) -> Vec<Rect> {
        let available = area.width as u32;
        let available_after_gap = available.saturating_sub(total_gap);
        let sizes = self.resolve_sizes(available_after_gap, area.width);

        let mut rects = Vec::with_capacity(sizes.len());
        let mut x = area.x;
        for (i, &size) in sizes.iter().enumerate() {
            let width = size.min(area.x + area.width - x);
            if width == 0 {
                break;
            }
            rects.push(Rect::new(x, area.y, width, area.height));
            x += width;
            if i + 1 < sizes.len() {
                x += self.gap;
            }
        }
        rects
    }

    /// Resolve constraints into actual sizes given available space.
    fn resolve_sizes(&self, available: u32, _total: u16) -> Vec<u16> {
        let n = self.constraints.len();
        let mut sizes = vec![0u16; n];
        let mut remaining = available;

        // First pass: resolve Length and Max constraints
        for (i, constraint) in self.constraints.iter().enumerate() {
            match constraint {
                Constraint::Length(len) => {
                    let resolved = (*len as u32).min(remaining);
                    sizes[i] = resolved as u16;
                    remaining = remaining.saturating_sub(resolved);
                }
                Constraint::Max(max) => {
                    let resolved = (*max as u32).min(remaining);
                    sizes[i] = resolved as u16;
                    remaining = remaining.saturating_sub(resolved);
                }
                _ => {}
            }
        }

        // Second pass: resolve Percentage constraints based on the total
        // available space (before Length/Max), not the remaining space.
        // This matches how TUI frameworks like ratatui handle percentages:
        // percentages are of the total, not of what's left after fixed items.
        let percentage_base = available;
        for (i, constraint) in self.constraints.iter().enumerate() {
            if let Constraint::Percentage(pct) = constraint {
                let resolved = (percentage_base * *pct as u32 / 100).min(remaining);
                sizes[i] = resolved as u16;
                remaining = remaining.saturating_sub(resolved);
            }
        }

        // Third pass: resolve Min constraints from remaining space
        for (i, constraint) in self.constraints.iter().enumerate() {
            if let Constraint::Min(min) = constraint {
                let resolved = (*min as u32).min(remaining);
                sizes[i] = sizes[i].max(resolved as u16);
                remaining = remaining.saturating_sub(resolved);
            }
        }

        // Distribute any remaining space to zero-sized areas
        if remaining > 0 {
            let zero_count = sizes.iter().filter(|&&s| s == 0).count() as u32;
            if zero_count > 0 {
                let per_area = remaining / zero_count.max(1);
                for size in sizes.iter_mut() {
                    if *size == 0 {
                        *size = per_area as u16;
                        remaining = remaining.saturating_sub(per_area);
                    }
                }
            }
        }

        sizes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_new() {
        let r = Rect::new(1, 2, 30, 10);
        assert_eq!(r.x, 1);
        assert_eq!(r.y, 2);
        assert_eq!(r.width, 30);
        assert_eq!(r.height, 10);
    }

    #[test]
    fn test_rect_area() {
        let r = Rect::new(0, 0, 30, 10);
        assert_eq!(r.area(), 300);
    }

    #[test]
    fn test_rect_is_empty() {
        assert!(Rect::new(0, 0, 0, 10).is_empty());
        assert!(Rect::new(0, 0, 10, 0).is_empty());
        assert!(!Rect::new(0, 0, 10, 10).is_empty());
    }

    #[test]
    fn test_rect_contains() {
        let r = Rect::new(5, 5, 10, 10);
        assert!(r.contains(5, 5));
        assert!(r.contains(14, 14));
        assert!(!r.contains(4, 5));
        assert!(!r.contains(15, 5));
        assert!(!r.contains(5, 15));
    }

    #[test]
    fn test_rect_shrink() {
        let r = Rect::new(0, 0, 10, 10).shrink(1);
        assert_eq!(r.x, 1);
        assert_eq!(r.y, 1);
        assert_eq!(r.width, 8);
        assert_eq!(r.height, 8);
    }

    #[test]
    fn test_rect_inner() {
        let r = Rect::new(0, 0, 10, 10).inner();
        assert_eq!(r.x, 1);
        assert_eq!(r.y, 1);
        assert_eq!(r.width, 8);
        assert_eq!(r.height, 8);
    }

    #[test]
    fn test_rect_split_horizontally() {
        let r = Rect::new(0, 0, 80, 24);
        let (top, bottom) = r.split_horizontally(6);
        assert_eq!(top, Rect::new(0, 0, 80, 6));
        assert_eq!(bottom, Rect::new(0, 6, 80, 18));
    }

    #[test]
    fn test_rect_split_vertically() {
        let r = Rect::new(0, 0, 80, 24);
        let (left, right) = r.split_vertically(60);
        assert_eq!(left, Rect::new(0, 0, 60, 24));
        assert_eq!(right, Rect::new(60, 0, 20, 24));
    }

    #[test]
    fn test_layout_vertical_fixed() {
        let layout = Layout::new(Direction::Vertical).constraints(vec![
            Constraint::Length(1),
            Constraint::Length(20),
            Constraint::Length(3),
        ]);
        let area = Rect::new(0, 0, 80, 24);
        let rects = layout.split(area);

        assert_eq!(rects.len(), 3);
        assert_eq!(rects[0], Rect::new(0, 0, 80, 1)); // status bar
        assert_eq!(rects[1], Rect::new(0, 1, 80, 20)); // main area
        assert_eq!(rects[2], Rect::new(0, 21, 80, 3)); // input bar
    }

    #[test]
    fn test_layout_horizontal_percentage() {
        let layout = Layout::new(Direction::Horizontal)
            .constraints(vec![Constraint::Percentage(70), Constraint::Percentage(30)]);
        let area = Rect::new(0, 0, 100, 24);
        let rects = layout.split(area);

        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].width, 70);
        assert_eq!(rects[1].width, 30);
    }

    #[test]
    fn test_layout_with_gap() {
        let layout = Layout::new(Direction::Horizontal)
            .constraints(vec![Constraint::Length(60), Constraint::Length(30)])
            .gap(1);
        let area = Rect::new(0, 0, 91, 24);
        let rects = layout.split(area);

        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0], Rect::new(0, 0, 60, 24));
        assert_eq!(rects[1], Rect::new(61, 0, 30, 24));
    }

    #[test]
    fn test_layout_nested() {
        // Simulate the TinyHarness TUI layout:
        // Top:    status bar (1 row)
        // Middle: conversation (70%) + sidebar (30%)
        // Bottom: input bar (3 rows)

        let full = Rect::new(0, 0, 80, 24);
        let main_layout = Layout::new(Direction::Vertical).constraints(vec![
            Constraint::Length(1), // status bar
            Constraint::Min(0),    // main area (takes remaining)
            Constraint::Length(3), // input bar
        ]);
        let rects = main_layout.split(full);

        assert_eq!(rects[0].height, 1); // status bar
        assert_eq!(rects[2].height, 3); // input bar
        assert_eq!(rects[1].height, 20); // main area

        // Split main area horizontally (80 columns wide)
        // 70% of 80 = 56, 30% of 80 = 24
        let side_layout = Layout::new(Direction::Horizontal)
            .constraints(vec![Constraint::Percentage(70), Constraint::Percentage(30)]);
        let side_rects = side_layout.split(rects[1]);

        assert_eq!(side_rects[0].width, 56); // 70% of 80
        assert_eq!(side_rects[1].width, 24); // 30% of 80
    }

    #[test]
    fn test_layout_empty_constraints() {
        let layout = Layout::new(Direction::Vertical).constraints(vec![]);
        let area = Rect::new(0, 0, 80, 24);
        let rects = layout.split(area);
        assert!(rects.is_empty());
    }

    #[test]
    fn test_layout_empty_area() {
        let layout = Layout::new(Direction::Vertical).constraints(vec![Constraint::Length(10)]);
        let area = Rect::new(0, 0, 0, 0);
        let rects = layout.split(area);
        assert!(rects.is_empty());
    }
}
