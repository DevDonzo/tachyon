use std::collections::HashSet;
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
        let max_top = self.max_top_line(total_lines);
        let unclamped = self.top_line.0 as i64 + delta;
        self.top_line = LineNumber(unclamped.clamp(0, max_top as i64) as u64);
    }

    pub fn jump_to_line(&mut self, target_line: LineNumber, total_lines: u64) {
        self.top_line = LineNumber(target_line.0.min(self.max_top_line(total_lines)));
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

    fn max_top_line(&self, total_lines: u64) -> u64 {
        total_lines.saturating_sub(self.visible_rows as u64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HighlightKind {
    Search,
    Selection,
    Diagnostic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HighlightSpan {
    pub line: LineNumber,
    pub start_col: u32,
    pub end_col: u32,
    pub kind: HighlightKind,
}

impl HighlightSpan {
    pub fn new(line: LineNumber, start_col: u32, end_col: u32, kind: HighlightKind) -> Option<Self> {
        (end_col > start_col).then_some(Self {
            line,
            start_col,
            end_col,
            kind,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HighlightBatch {
    pub line: LineNumber,
    pub start_col: u32,
    pub end_col: u32,
    pub kind: HighlightKind,
    pub span_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramePlan {
    pub frame_number: u64,
    pub visible_line_range: Range<LineNumber>,
    pub upload_line_ranges: Vec<Range<LineNumber>>,
    pub highlight_batches: Vec<HighlightBatch>,
    pub dirty_glyphs: Vec<char>,
    pub text_instance_count: usize,
}

#[derive(Debug, Default, Clone)]
pub struct GlyphAtlasState {
    known_glyphs: HashSet<char>,
    dirty_queue: Vec<char>,
}

impl GlyphAtlasState {
    pub fn queue_text(&mut self, text: &str) {
        for glyph in text.chars() {
            if self.known_glyphs.insert(glyph) {
                self.dirty_queue.push(glyph);
            }
        }
    }

    pub fn drain_dirty(&mut self, max_glyph_uploads: usize) -> Vec<char> {
        let count = max_glyph_uploads.min(self.dirty_queue.len());
        self.dirty_queue.drain(..count).collect()
    }
}

#[derive(Debug, Clone)]
pub struct RenderPipelineState {
    frame_number: u64,
    previous_visible: Option<Range<LineNumber>>,
    glyph_atlas: GlyphAtlasState,
    max_glyph_uploads_per_frame: usize,
}

impl RenderPipelineState {
    pub fn new(max_glyph_uploads_per_frame: usize) -> Self {
        Self {
            frame_number: 0,
            previous_visible: None,
            glyph_atlas: GlyphAtlasState::default(),
            max_glyph_uploads_per_frame: max_glyph_uploads_per_frame.max(1),
        }
    }

    pub fn plan_frame<'a>(
        &mut self,
        visible_line_range: Range<LineNumber>,
        visible_line_texts: impl IntoIterator<Item = &'a str>,
        highlights: &[HighlightSpan],
    ) -> FramePlan {
        let visible_line_texts = visible_line_texts.into_iter().collect::<Vec<_>>();
        for text in &visible_line_texts {
            self.glyph_atlas.queue_text(text);
        }

        let upload_line_ranges =
            compute_upload_ranges(self.previous_visible.as_ref(), &visible_line_range);
        let dirty_glyphs = self
            .glyph_atlas
            .drain_dirty(self.max_glyph_uploads_per_frame);
        let highlight_batches = batch_highlights(highlights);

        self.frame_number += 1;
        self.previous_visible = Some(visible_line_range.clone());

        FramePlan {
            frame_number: self.frame_number,
            visible_line_range,
            upload_line_ranges,
            highlight_batches,
            dirty_glyphs,
            text_instance_count: visible_line_texts.len(),
        }
    }
}

fn compute_upload_ranges(
    previous_visible: Option<&Range<LineNumber>>,
    current_visible: &Range<LineNumber>,
) -> Vec<Range<LineNumber>> {
    if current_visible.start.0 >= current_visible.end.0 {
        return Vec::new();
    }

    let Some(previous_visible) = previous_visible else {
        return vec![current_visible.start..current_visible.end];
    };

    if current_visible.end.0 <= previous_visible.start.0
        || current_visible.start.0 >= previous_visible.end.0
    {
        return vec![current_visible.start..current_visible.end];
    }

    let mut ranges = Vec::new();
    if current_visible.start.0 < previous_visible.start.0 {
        ranges.push(current_visible.start..LineNumber(previous_visible.start.0));
    }
    if current_visible.end.0 > previous_visible.end.0 {
        ranges.push(LineNumber(previous_visible.end.0)..current_visible.end);
    }
    ranges
}

fn batch_highlights(highlights: &[HighlightSpan]) -> Vec<HighlightBatch> {
    if highlights.is_empty() {
        return Vec::new();
    }

    let mut sorted = highlights.to_vec();
    sorted.sort_by_key(|highlight| {
        (
            highlight.line.0,
            highlight.start_col,
            highlight.end_col,
            highlight_kind_priority(highlight.kind),
        )
    });

    let mut batches = Vec::new();
    let mut current = HighlightBatch {
        line: sorted[0].line,
        start_col: sorted[0].start_col,
        end_col: sorted[0].end_col,
        kind: sorted[0].kind,
        span_count: 1,
    };

    for highlight in sorted.into_iter().skip(1) {
        let can_merge = current.line == highlight.line
            && current.kind == highlight.kind
            && highlight.start_col <= current.end_col.saturating_add(1);
        if can_merge {
            current.end_col = current.end_col.max(highlight.end_col);
            current.span_count += 1;
        } else {
            batches.push(current);
            current = HighlightBatch {
                line: highlight.line,
                start_col: highlight.start_col,
                end_col: highlight.end_col,
                kind: highlight.kind,
                span_count: 1,
            };
        }
    }
    batches.push(current);
    batches
}

fn highlight_kind_priority(kind: HighlightKind) -> u8 {
    match kind {
        HighlightKind::Search => 0,
        HighlightKind::Selection => 1,
        HighlightKind::Diagnostic => 2,
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

    #[test]
    fn jump_to_line_is_clamped() {
        let mut viewport = Viewport::new(10, 3);
        viewport.jump_to_line(LineNumber(42), 30);
        assert_eq!(viewport.top_line, LineNumber(20));
    }

    #[test]
    fn frame_plan_uploads_only_newly_visible_lines() {
        let mut pipeline = RenderPipelineState::new(8);
        let first = pipeline.plan_frame(LineNumber(0)..LineNumber(5), ["alpha", "beta"], &[]);
        assert_eq!(first.upload_line_ranges, vec![LineNumber(0)..LineNumber(5)]);

        let second = pipeline.plan_frame(LineNumber(2)..LineNumber(7), ["alpha", "beta"], &[]);
        assert_eq!(second.upload_line_ranges, vec![LineNumber(5)..LineNumber(7)]);
    }

    #[test]
    fn frame_plan_respects_glyph_upload_budget() {
        let mut pipeline = RenderPipelineState::new(2);
        let first = pipeline.plan_frame(LineNumber(0)..LineNumber(1), ["abcdef"], &[]);
        let second = pipeline.plan_frame(LineNumber(0)..LineNumber(1), ["abcdef"], &[]);
        let third = pipeline.plan_frame(LineNumber(0)..LineNumber(1), ["abcdef"], &[]);
        assert_eq!(first.dirty_glyphs.len(), 2);
        assert_eq!(second.dirty_glyphs.len(), 2);
        assert_eq!(third.dirty_glyphs.len(), 2);
    }

    #[test]
    fn highlight_batching_merges_adjacent_ranges() {
        let highlights = vec![
            HighlightSpan::new(LineNumber(8), 4, 6, HighlightKind::Search).unwrap(),
            HighlightSpan::new(LineNumber(8), 7, 10, HighlightKind::Search).unwrap(),
            HighlightSpan::new(LineNumber(8), 12, 16, HighlightKind::Selection).unwrap(),
            HighlightSpan::new(LineNumber(9), 1, 2, HighlightKind::Search).unwrap(),
        ];
        let batches = batch_highlights(&highlights);
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0].line, LineNumber(8));
        assert_eq!(batches[0].start_col, 4);
        assert_eq!(batches[0].end_col, 10);
        assert_eq!(batches[0].span_count, 2);
    }
}
