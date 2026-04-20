use std::ops::Range;
use tachyon_core::LineNumber;

#[derive(Debug, Clone)]
pub struct Viewport {
    pub top_line: LineNumber,
    pub visible_rows: u32,
    pub overscan_rows: u32,
}

impl Viewport {
    pub fn new(visible_rows: u32, overscan_rows: u32) -> Self {
        Self {
            top_line: LineNumber(0),
            visible_rows: visible_rows.max(1),
            overscan_rows,
        }
    }

    pub fn scroll_lines(&mut self, delta: i64, total_lines: u64) {
        let max_top = total_lines.saturating_sub(self.visible_rows as u64);
        let unclamped = self.top_line.0 as i64 + delta;
        self.top_line = LineNumber(unclamped.clamp(0, max_top as i64) as u64);
    }

    pub fn visible_line_range(&self, total_lines: u64) -> Range<LineNumber> {
        let start = self.top_line.0.min(total_lines);
        let end = (start + self.visible_rows as u64).min(total_lines);
        LineNumber(start)..LineNumber(end)
    }

    pub fn fetch_line_range(&self, total_lines: u64) -> Range<LineNumber> {
        let overscan = self.overscan_rows as u64;
        let start = self.top_line.0.saturating_sub(overscan).min(total_lines);
        let end = (self.top_line.0 + self.visible_rows as u64 + overscan).min(total_lines);
        LineNumber(start)..LineNumber(end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_range_clamps_to_total_lines() {
        let viewport = Viewport::new(5, 2);
        assert_eq!(viewport.visible_line_range(3), LineNumber(0)..LineNumber(3));
    }

    #[test]
    fn scroll_is_bounded() {
        let mut viewport = Viewport::new(10, 3);
        viewport.scroll_lines(100, 50);
        assert_eq!(viewport.top_line, LineNumber(40));
        viewport.scroll_lines(-500, 50);
        assert_eq!(viewport.top_line, LineNumber(0));
    }

    #[test]
    fn fetch_range_includes_overscan() {
        let mut viewport = Viewport::new(10, 5);
        viewport.scroll_lines(20, 100);
        assert_eq!(
            viewport.fetch_line_range(100),
            LineNumber(15)..LineNumber(35)
        );
    }
}
