use crossbeam_channel::unbounded;
use rayon::prelude::*;
use regex::bytes::Regex;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tachyon_core::{
    ByteOffset, ByteRange, LineNumber, Result, SearchMode, SearchQuery, TachyonError,
};
use tachyon_ingest::NewlineIndex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub line: LineNumber,
    pub byte_range: ByteRange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStage {
    Visible,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchBatch {
    pub stage: SearchStage,
    pub line_range: Range<LineNumber>,
    pub hits: Vec<SearchHit>,
}

#[derive(Debug, Clone)]
pub struct SearchConfig {
    pub visible_lines: Range<LineNumber>,
    pub chunk_lines: u64,
    pub max_hits: usize,
    pub batch_hit_target: usize,
}

impl SearchConfig {
    pub fn with_visible_lines(visible_lines: Range<LineNumber>) -> Self {
        Self {
            visible_lines,
            chunk_lines: 8_192,
            max_hits: 5_000,
            batch_hit_target: 256,
        }
    }
}

enum CompiledQuery {
    Substring {
        needle: Vec<u8>,
        case_sensitive: bool,
    },
    Regex(Regex),
}

impl CompiledQuery {
    fn compile(query: &SearchQuery) -> Result<Self> {
        match &query.mode {
            SearchMode::Substring { case_sensitive } => {
                if query.pattern.is_empty() {
                    return Err(TachyonError::InvalidQuery(
                        "substring pattern must not be empty".to_owned(),
                    ));
                }
                Ok(Self::Substring {
                    needle: query.pattern.as_bytes().to_vec(),
                    case_sensitive: *case_sensitive,
                })
            }
            SearchMode::Regex => {
                let regex = Regex::new(&query.pattern)
                    .map_err(|error| TachyonError::InvalidQuery(error.to_string()))?;
                Ok(Self::Regex(regex))
            }
        }
    }

    fn find_offsets(&self, line_bytes: &[u8]) -> Vec<Range<usize>> {
        match self {
            Self::Substring {
                needle,
                case_sensitive,
            } => find_substring_offsets(line_bytes, needle, *case_sensitive),
            Self::Regex(regex) => regex
                .find_iter(line_bytes)
                .map(|found| found.start()..found.end())
                .collect(),
        }
    }
}

fn find_substring_offsets(
    haystack: &[u8],
    needle: &[u8],
    case_sensitive: bool,
) -> Vec<Range<usize>> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }

    if case_sensitive {
        return memchr::memmem::find_iter(haystack, needle)
            .map(|start| start..start + needle.len())
            .collect();
    }

    let haystack_storage = haystack
        .iter()
        .map(u8::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let needle_storage = needle
        .iter()
        .map(u8::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let (haystack_ref, needle_ref): (&[u8], &[u8]) = (&haystack_storage, &needle_storage);

    let mut ranges = Vec::new();
    let mut start = 0usize;
    while start + needle_ref.len() <= haystack_ref.len() {
        if &haystack_ref[start..start + needle_ref.len()] == needle_ref {
            ranges.push(start..start + needle_ref.len());
        }
        start += 1;
    }
    ranges
}

pub fn search_streaming(
    data: &[u8],
    index: &NewlineIndex,
    query: &SearchQuery,
    config: &SearchConfig,
    cancelled: &AtomicBool,
    mut on_batch: impl FnMut(SearchBatch),
) -> Result<usize> {
    let compiled = CompiledQuery::compile(query)?;
    let total_lines = index.total_lines();
    let visible = clamp_line_range(config.visible_lines.clone(), total_lines)?;
    let chunk_lines = config.chunk_lines.max(1);
    let batch_hit_target = config.batch_hit_target.max(1);
    let max_hits = config.max_hits;
    if max_hits == 0 {
        return Ok(0);
    }

    let remaining = AtomicUsize::new(max_hits);
    let mut emitted_hits = 0usize;

    let scan_context = SearchScanContext {
        data,
        index,
        compiled: &compiled,
        remaining: &remaining,
        cancelled,
        batch_hit_target,
    };

    stream_line_range(
        &scan_context,
        visible.start.0..visible.end.0,
        SearchStage::Visible,
        |batch| {
            emitted_hits += batch.hits.len();
            on_batch(batch);
        },
    )?;

    if cancelled.load(Ordering::Relaxed) || remaining.load(Ordering::Relaxed) == 0 {
        return Ok(emitted_hits);
    }

    let (sender, receiver) = unbounded::<SearchBatch>();
    let error_slot = Arc::new(Mutex::new(None::<TachyonError>));
    let chunk_ranges = build_background_chunks(total_lines, visible, chunk_lines);

    std::thread::scope(|scope| {
        let error_slot = Arc::clone(&error_slot);
        scope.spawn(move || {
            chunk_ranges
                .into_par_iter()
                .for_each_with((sender, error_slot), |state, chunk| {
                    let (sender, error_slot) = state;
                    if cancelled.load(Ordering::Relaxed) || remaining.load(Ordering::Relaxed) == 0 {
                        return;
                    }

                    match collect_chunk_hits(
                        data,
                        index,
                        &compiled,
                        chunk.clone(),
                        &remaining,
                        cancelled,
                    ) {
                        Ok(hits) if !hits.is_empty() => {
                            let batch = SearchBatch {
                                stage: SearchStage::Background,
                                line_range: LineNumber(chunk.start)..LineNumber(chunk.end),
                                hits,
                            };
                            let _ = sender.send(batch);
                        }
                        Ok(_) => {}
                        Err(error) => {
                            if let Ok(mut slot) = error_slot.lock() {
                                *slot = Some(error);
                            }
                            cancelled.store(true, Ordering::Relaxed);
                        }
                    }
                });
        });

        while let Ok(batch) = receiver.recv() {
            emitted_hits += batch.hits.len();
            on_batch(batch);
        }
    });

    if let Ok(mut slot) = error_slot.lock()
        && let Some(error) = slot.take()
    {
        return Err(error);
    }

    Ok(emitted_hits)
}

pub fn search_visible_first(
    data: &[u8],
    index: &NewlineIndex,
    query: &SearchQuery,
    visible_lines: Range<LineNumber>,
    max_hits: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<SearchHit>> {
    let config = SearchConfig {
        visible_lines,
        chunk_lines: 8_192,
        max_hits,
        batch_hit_target: max_hits.max(1),
    };
    let mut hits = Vec::new();
    let _ = search_streaming(data, index, query, &config, cancelled, |batch| {
        hits.extend(batch.hits);
    })?;
    Ok(hits)
}

fn clamp_line_range(range: Range<LineNumber>, total_lines: u64) -> Result<Range<LineNumber>> {
    if range.start.0 > range.end.0 {
        return Err(TachyonError::InvalidLineRange {
            start: range.start.0,
            end: range.end.0,
            total: total_lines,
        });
    }

    let start = range.start.0.min(total_lines);
    let end = range.end.0.min(total_lines);
    Ok(LineNumber(start)..LineNumber(end))
}

fn build_background_chunks(
    total_lines: u64,
    visible: Range<LineNumber>,
    chunk_lines: u64,
) -> Vec<Range<u64>> {
    let mut chunks = Vec::new();
    push_chunks(0, visible.start.0, chunk_lines, &mut chunks);
    push_chunks(visible.end.0, total_lines, chunk_lines, &mut chunks);
    chunks
}

fn push_chunks(start: u64, end: u64, chunk_lines: u64, output: &mut Vec<Range<u64>>) {
    if start >= end {
        return;
    }

    let mut line = start;
    while line < end {
        let chunk_end = (line + chunk_lines).min(end);
        output.push(line..chunk_end);
        line = chunk_end;
    }
}

struct SearchScanContext<'a> {
    data: &'a [u8],
    index: &'a NewlineIndex,
    compiled: &'a CompiledQuery,
    remaining: &'a AtomicUsize,
    cancelled: &'a AtomicBool,
    batch_hit_target: usize,
}

fn stream_line_range(
    context: &SearchScanContext<'_>,
    line_range: Range<u64>,
    stage: SearchStage,
    mut on_batch: impl FnMut(SearchBatch),
) -> Result<()> {
    let start_line = line_range.start;
    let end_line = line_range.end;
    if start_line >= end_line {
        return Ok(());
    }

    if let CompiledQuery::Substring {
        needle,
        case_sensitive: true,
    } = context.compiled
    {
        return stream_substring_range(context, start_line..end_line, needle, stage, on_batch);
    }

    let mut batch_hits = Vec::with_capacity(context.batch_hit_target);
    let mut batch_start = None::<u64>;
    let mut current_line = start_line;

    while current_line < end_line {
        if context.cancelled.load(Ordering::Relaxed)
            || context.remaining.load(Ordering::Relaxed) == 0
        {
            break;
        }

        let line = LineNumber(current_line);
        let range = context.index.line_byte_range(line)?;
        let line_bytes = &context.data[range.start.0 as usize..range.end.0 as usize];
        for local in context.compiled.find_offsets(line_bytes) {
            if !try_take_hit_slot(context.remaining) {
                break;
            }
            if batch_start.is_none() {
                batch_start = Some(current_line);
            }
            let byte_range = ByteRange::new(
                ByteOffset(range.start.0 + local.start as u64),
                ByteOffset(range.start.0 + local.end as u64),
            )?;
            batch_hits.push(SearchHit { line, byte_range });
            if batch_hits.len() >= context.batch_hit_target {
                let range_start = batch_start.unwrap_or(current_line);
                on_batch(SearchBatch {
                    stage,
                    line_range: LineNumber(range_start)..LineNumber(current_line + 1),
                    hits: std::mem::take(&mut batch_hits),
                });
                batch_start = None;
            }
        }
        current_line += 1;
    }

    if !batch_hits.is_empty() {
        let range_start = batch_start.unwrap_or(start_line);
        on_batch(SearchBatch {
            stage,
            line_range: LineNumber(range_start)..LineNumber(current_line),
            hits: batch_hits,
        });
    }

    Ok(())
}

fn collect_chunk_hits(
    data: &[u8],
    index: &NewlineIndex,
    compiled: &CompiledQuery,
    chunk: Range<u64>,
    remaining: &AtomicUsize,
    cancelled: &AtomicBool,
) -> Result<Vec<SearchHit>> {
    if let CompiledQuery::Substring {
        needle,
        case_sensitive: true,
    } = compiled
    {
        return collect_substring_range_hits(data, index, chunk, needle, remaining, cancelled);
    }

    let mut hits = Vec::new();
    for line_idx in chunk {
        if cancelled.load(Ordering::Relaxed) || remaining.load(Ordering::Relaxed) == 0 {
            break;
        }

        let line = LineNumber(line_idx);
        let range = index.line_byte_range(line)?;
        let line_bytes = &data[range.start.0 as usize..range.end.0 as usize];
        for local in compiled.find_offsets(line_bytes) {
            if !try_take_hit_slot(remaining) {
                return Ok(hits);
            }
            let byte_range = ByteRange::new(
                ByteOffset(range.start.0 + local.start as u64),
                ByteOffset(range.start.0 + local.end as u64),
            )?;
            hits.push(SearchHit { line, byte_range });
        }
    }
    Ok(hits)
}

fn stream_substring_range(
    context: &SearchScanContext<'_>,
    line_range: Range<u64>,
    needle: &[u8],
    stage: SearchStage,
    mut on_batch: impl FnMut(SearchBatch),
) -> Result<()> {
    let byte_range = line_range_byte_bounds(context.index, line_range.clone())?;
    if byte_range.start >= byte_range.end {
        return Ok(());
    }

    let mut batch_hits = Vec::with_capacity(context.batch_hit_target);
    let mut batch_start = None::<u64>;
    let mut current_line = line_range.start;
    let mut current_line_range = context.index.line_byte_range(LineNumber(current_line))?;
    let base = byte_range.start;
    let bytes = &context.data[byte_range.start as usize..byte_range.end as usize];

    for local_start in memchr::memmem::find_iter(bytes, needle) {
        if context.cancelled.load(Ordering::Relaxed) {
            break;
        }

        let absolute_start = base + local_start as u64;
        let absolute_end = absolute_start + needle.len() as u64;
        while current_line + 1 < line_range.end && absolute_start > current_line_range.end.0 {
            current_line += 1;
            current_line_range = context.index.line_byte_range(LineNumber(current_line))?;
        }

        if absolute_start < current_line_range.start.0 || absolute_end > current_line_range.end.0 {
            continue;
        }
        if !try_take_hit_slot(context.remaining) {
            break;
        }

        if batch_start.is_none() {
            batch_start = Some(current_line);
        }

        batch_hits.push(SearchHit {
            line: LineNumber(current_line),
            byte_range: ByteRange::new(ByteOffset(absolute_start), ByteOffset(absolute_end))?,
        });

        if batch_hits.len() >= context.batch_hit_target {
            let range_start = batch_start.unwrap_or(current_line);
            on_batch(SearchBatch {
                stage,
                line_range: LineNumber(range_start)..LineNumber(current_line + 1),
                hits: std::mem::take(&mut batch_hits),
            });
            batch_start = None;
        }
    }

    if !batch_hits.is_empty() {
        let range_start = batch_start.unwrap_or(line_range.start);
        on_batch(SearchBatch {
            stage,
            line_range: LineNumber(range_start)..LineNumber(current_line + 1),
            hits: batch_hits,
        });
    }

    Ok(())
}

fn collect_substring_range_hits(
    data: &[u8],
    index: &NewlineIndex,
    line_range: Range<u64>,
    needle: &[u8],
    remaining: &AtomicUsize,
    cancelled: &AtomicBool,
) -> Result<Vec<SearchHit>> {
    let byte_range = line_range_byte_bounds(index, line_range.clone())?;
    if byte_range.start >= byte_range.end {
        return Ok(Vec::new());
    }

    let mut hits = Vec::new();
    let base = byte_range.start;
    let bytes = &data[byte_range.start as usize..byte_range.end as usize];
    let mut current_line = line_range.start;
    let mut current_line_range = index.line_byte_range(LineNumber(current_line))?;

    for local_start in memchr::memmem::find_iter(bytes, needle) {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }

        let absolute_start = base + local_start as u64;
        let absolute_end = absolute_start + needle.len() as u64;
        while current_line + 1 < line_range.end && absolute_start > current_line_range.end.0 {
            current_line += 1;
            current_line_range = index.line_byte_range(LineNumber(current_line))?;
        }

        if absolute_start < current_line_range.start.0 || absolute_end > current_line_range.end.0 {
            continue;
        }
        if !try_take_hit_slot(remaining) {
            break;
        }

        hits.push(SearchHit {
            line: LineNumber(current_line),
            byte_range: ByteRange::new(ByteOffset(absolute_start), ByteOffset(absolute_end))?,
        });
    }
    Ok(hits)
}

fn line_range_byte_bounds(index: &NewlineIndex, line_range: Range<u64>) -> Result<Range<u64>> {
    if line_range.start >= line_range.end {
        return Ok(0..0);
    }

    let start = index.line_to_byte(LineNumber(line_range.start))?.0;
    let end = if line_range.end >= index.total_lines() {
        index.file_len()
    } else {
        index
            .line_to_byte(LineNumber(line_range.end))?
            .0
            .saturating_sub(1)
    };
    Ok(start..end)
}

fn try_take_hit_slot(remaining: &AtomicUsize) -> bool {
    remaining
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |slots| {
            if slots > 0 { Some(slots - 1) } else { None }
        })
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use tachyon_core::SearchQuery;

    #[test]
    fn streaming_emits_visible_hits_before_background_hits() {
        let data = b"alpha\nbeta\nalpha\nbeta\n";
        let index = NewlineIndex::from_bytes_parallel(data, 4);
        let query = SearchQuery::substring("beta", true).unwrap();
        let config = SearchConfig {
            visible_lines: LineNumber(1)..LineNumber(2),
            chunk_lines: 1,
            max_hits: 10,
            batch_hit_target: 1,
        };
        let cancelled = AtomicBool::new(false);
        let mut stages = Vec::new();
        let mut lines = Vec::new();
        let emitted = search_streaming(data, &index, &query, &config, &cancelled, |batch| {
            stages.push(batch.stage);
            lines.extend(batch.hits.into_iter().map(|hit| hit.line));
        })
        .unwrap();

        assert_eq!(emitted, 2);
        assert_eq!(stages[0], SearchStage::Visible);
        assert_eq!(lines[0], LineNumber(1));
        assert!(lines.contains(&LineNumber(3)));
    }

    #[test]
    fn cancellation_stops_search() {
        let data = b"alpha\nbeta\nalpha\nbeta\n";
        let index = NewlineIndex::from_bytes_parallel(data, 4);
        let query = SearchQuery::substring("beta", true).unwrap();
        let config =
            SearchConfig::with_visible_lines(LineNumber(0)..LineNumber(index.total_lines()));
        let cancelled = AtomicBool::new(true);

        let emitted = search_streaming(data, &index, &query, &config, &cancelled, |_| {}).unwrap();
        assert_eq!(emitted, 0);
    }

    #[test]
    fn max_hits_is_respected() {
        let data = b"beta\nbeta\nbeta\nbeta\nbeta\n";
        let index = NewlineIndex::from_bytes_parallel(data, 2);
        let query = SearchQuery::substring("beta", true).unwrap();
        let config = SearchConfig {
            visible_lines: LineNumber(0)..LineNumber(index.total_lines()),
            chunk_lines: 2,
            max_hits: 3,
            batch_hit_target: 1,
        };
        let cancelled = AtomicBool::new(false);
        let mut collected = Vec::new();

        let emitted = search_streaming(data, &index, &query, &config, &cancelled, |batch| {
            collected.extend(batch.hits);
        })
        .unwrap();

        assert_eq!(emitted, 3);
        assert_eq!(collected.len(), 3);
    }

    #[test]
    fn case_insensitive_substring_matches() {
        let data = b"Service=API\nservice=api\n";
        let index = NewlineIndex::from_bytes_parallel(data, 8);
        let query = SearchQuery::substring("service=api", false).unwrap();
        let cancelled = AtomicBool::new(false);

        let hits = search_visible_first(
            data,
            &index,
            &query,
            LineNumber(0)..LineNumber(index.total_lines()),
            10,
            &cancelled,
        )
        .unwrap();

        assert_eq!(hits.len(), 2);
    }
}
