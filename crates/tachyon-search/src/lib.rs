use regex::bytes::Regex;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use tachyon_core::{
    ByteOffset, ByteRange, LineNumber, Result, SearchMode, SearchQuery, TachyonError,
};
use tachyon_ingest::NewlineIndex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub line: LineNumber,
    pub byte_range: ByteRange,
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
                .map(|match_range| match_range.start()..match_range.end())
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

    let (haystack, needle_storage, needle) = if case_sensitive {
        (haystack.to_vec(), Vec::new(), needle.to_vec())
    } else {
        let lowered_haystack: Vec<u8> = haystack.iter().map(u8::to_ascii_lowercase).collect();
        let lowered_needle: Vec<u8> = needle.iter().map(u8::to_ascii_lowercase).collect();
        (lowered_haystack, lowered_needle.clone(), lowered_needle)
    };

    let needle_ref = if case_sensitive {
        needle.as_slice()
    } else {
        needle_storage.as_slice()
    };
    let mut ranges = Vec::new();
    let mut start = 0usize;
    while start + needle_ref.len() <= haystack.len() {
        if &haystack[start..start + needle_ref.len()] == needle_ref {
            ranges.push(start..start + needle_ref.len());
        }
        start += 1;
    }
    ranges
}

pub fn search_visible_first(
    data: &[u8],
    index: &NewlineIndex,
    query: &SearchQuery,
    visible_lines: Range<LineNumber>,
    max_hits: usize,
    cancelled: &AtomicBool,
) -> Result<Vec<SearchHit>> {
    let compiled = CompiledQuery::compile(query)?;
    let total_lines = index.total_lines();

    let visible_start = visible_lines.start.0.min(total_lines);
    let visible_end = visible_lines.end.0.min(total_lines);

    let line_order = (visible_start..visible_end)
        .chain(0..visible_start)
        .chain(visible_end..total_lines);

    let mut hits = Vec::new();
    for line_idx in line_order {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }

        let line = LineNumber(line_idx);
        let range = index.line_byte_range(line)?;
        let line_bytes = &data[range.start.0 as usize..range.end.0 as usize];

        for local in compiled.find_offsets(line_bytes) {
            let global_range = ByteRange::new(
                ByteOffset(range.start.0 + local.start as u64),
                ByteOffset(range.start.0 + local.end as u64),
            )?;
            hits.push(SearchHit {
                line,
                byte_range: global_range,
            });
            if hits.len() >= max_hits {
                return Ok(hits);
            }
        }
    }

    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use tachyon_core::SearchQuery;

    #[test]
    fn substring_search_prefers_visible_region_then_rest() {
        let data = b"alpha\nbeta\ngamma\nbeta\n";
        let index = NewlineIndex::from_bytes_parallel(data, 4);
        let query = SearchQuery::substring("beta", true).unwrap();
        let cancelled = AtomicBool::new(false);

        let hits = search_visible_first(
            data,
            &index,
            &query,
            LineNumber(2)..LineNumber(3),
            10,
            &cancelled,
        )
        .unwrap();

        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].line, LineNumber(1));
        assert_eq!(hits[1].line, LineNumber(3));
    }

    #[test]
    fn regex_search_returns_expected_match() {
        let data = b"svc=api latency=10ms\nsvc=db latency=42ms\n";
        let index = NewlineIndex::from_bytes_parallel(data, 8);
        let query = SearchQuery::regex(r"latency=\d+ms").unwrap();
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
        assert_eq!(hits[0].line, LineNumber(0));
        assert_eq!(hits[1].line, LineNumber(1));
    }
}
