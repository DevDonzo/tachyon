use serde::Deserialize;
use std::collections::BTreeMap;
use tachyon_core::{Result, Span, TachyonError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimelineSpan {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub service: String,
    pub name: String,
    pub start_ns: u64,
    pub end_ns: u64,
    pub lane: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackSummary {
    pub service: String,
    pub lanes: u32,
    pub span_count: usize,
}

#[derive(Debug, Clone)]
pub struct TraceIndex {
    spans: Vec<Span>,
    order_by_start: Vec<usize>,
    lane_by_span: Vec<u32>,
    lanes_per_service: BTreeMap<String, u32>,
}

impl TraceIndex {
    pub fn build(spans: Vec<Span>) -> Result<Self> {
        for span in &spans {
            if span.end_ns < span.start_ns {
                return Err(TachyonError::Parse(
                    "span end_ns must be >= start_ns".to_owned(),
                ));
            }
        }

        let mut order_by_start = (0..spans.len()).collect::<Vec<_>>();
        order_by_start.sort_by_key(|index| {
            let span = &spans[*index];
            (span.start_ns, span.end_ns)
        });

        let mut per_service = BTreeMap::<String, Vec<usize>>::new();
        for (index, span) in spans.iter().enumerate() {
            per_service.entry(span.service.clone()).or_default().push(index);
        }

        let mut lane_by_span = vec![0u32; spans.len()];
        let mut lanes_per_service = BTreeMap::new();

        for (service, indexes) in &mut per_service {
            indexes.sort_by_key(|index| {
                let span = &spans[*index];
                (span.start_ns, span.end_ns)
            });

            let mut lane_end_times = Vec::<u64>::new();
            for span_index in indexes {
                let span = &spans[*span_index];
                let lane = match lane_end_times
                    .iter()
                    .position(|lane_end| span.start_ns >= *lane_end)
                {
                    Some(existing_lane) => {
                        lane_end_times[existing_lane] = span.end_ns;
                        existing_lane as u32
                    }
                    None => {
                        lane_end_times.push(span.end_ns);
                        (lane_end_times.len() - 1) as u32
                    }
                };
                lane_by_span[*span_index] = lane;
            }

            lanes_per_service.insert(service.clone(), lane_end_times.len() as u32);
        }

        Ok(Self {
            spans,
            order_by_start,
            lane_by_span,
            lanes_per_service,
        })
    }

    pub fn span_count(&self) -> usize {
        self.spans.len()
    }

    pub fn track_count(&self) -> usize {
        self.lanes_per_service.len()
    }

    pub fn track_summaries(&self) -> Vec<TrackSummary> {
        let mut counts = BTreeMap::<String, usize>::new();
        for span in &self.spans {
            *counts.entry(span.service.clone()).or_default() += 1;
        }

        self.lanes_per_service
            .iter()
            .map(|(service, lanes)| TrackSummary {
                service: service.clone(),
                lanes: *lanes,
                span_count: counts.get(service).copied().unwrap_or(0),
            })
            .collect()
    }

    pub fn time_bounds(&self) -> Option<(u64, u64)> {
        let first = self
            .order_by_start
            .first()
            .and_then(|index| self.spans.get(*index))?;
        let max_end = self.spans.iter().map(|span| span.end_ns).max()?;
        Some((first.start_ns, max_end))
    }

    pub fn query_window(
        &self,
        start_ns: u64,
        end_ns: u64,
        max_spans: usize,
    ) -> Result<Vec<TimelineSpan>> {
        if end_ns < start_ns {
            return Err(TachyonError::Parse(
                "time window end_ns must be >= start_ns".to_owned(),
            ));
        }
        if max_spans == 0 {
            return Ok(Vec::new());
        }

        let cutoff = self
            .order_by_start
            .partition_point(|index| self.spans[*index].start_ns < end_ns);

        let mut result = Vec::new();
        for index in &self.order_by_start[..cutoff] {
            let span = &self.spans[*index];
            if span.end_ns <= start_ns {
                continue;
            }

            result.push(TimelineSpan {
                trace_id: span.trace_id.clone(),
                span_id: span.span_id.clone(),
                parent_span_id: span.parent_span_id.clone(),
                service: span.service.clone(),
                name: span.name.clone(),
                start_ns: span.start_ns,
                end_ns: span.end_ns,
                lane: self.lane_by_span[*index],
            });

            if result.len() >= max_spans {
                break;
            }
        }

        Ok(result)
    }
}

#[derive(Debug, Deserialize)]
struct RawSpan {
    trace_id: String,
    span_id: String,
    #[serde(default)]
    parent_span_id: Option<String>,
    service: String,
    name: String,
    start_ns: u64,
    end_ns: u64,
}

impl TryFrom<RawSpan> for Span {
    type Error = TachyonError;

    fn try_from(value: RawSpan) -> Result<Self> {
        if value.end_ns < value.start_ns {
            return Err(TachyonError::Parse(
                "span end_ns must be >= start_ns".to_owned(),
            ));
        }

        Ok(Span {
            trace_id: value.trace_id,
            span_id: value.span_id,
            parent_span_id: value.parent_span_id,
            service: value.service,
            name: value.name,
            start_ns: value.start_ns,
            end_ns: value.end_ns,
        })
    }
}

pub fn parse_spans_json(input: &[u8]) -> Result<Vec<Span>> {
    let trimmed = std::str::from_utf8(input)
        .map_err(|error| TachyonError::Parse(format!("invalid UTF-8 input: {error}")))?
        .trim();

    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    if trimmed.starts_with('[') {
        let raw: Vec<RawSpan> = serde_json::from_str(trimmed)
            .map_err(|error| TachyonError::Parse(format!("invalid JSON array: {error}")))?;
        return raw.into_iter().map(Span::try_from).collect();
    }

    trimmed
        .lines()
        .map(|line| {
            let raw: RawSpan = serde_json::from_str(line)
                .map_err(|error| TachyonError::Parse(format!("invalid JSON line: {error}")))?;
            Span::try_from(raw)
        })
        .collect()
}

pub fn spans_in_window(spans: &[Span], start_ns: u64, end_ns: u64) -> Result<Vec<&Span>> {
    if end_ns < start_ns {
        return Err(TachyonError::Parse(
            "time window end_ns must be >= start_ns".to_owned(),
        ));
    }

    Ok(spans
        .iter()
        .filter(|span| span.start_ns < end_ns && span.end_ns > start_ns)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_lines() {
        let data = br#"{"trace_id":"t1","span_id":"s1","service":"api","name":"request","start_ns":10,"end_ns":30}
{"trace_id":"t1","span_id":"s2","parent_span_id":"s1","service":"db","name":"query","start_ns":12,"end_ns":22}
"#;
        let spans = parse_spans_json(data).unwrap();
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[1].parent_span_id.as_deref(), Some("s1"));
    }

    #[test]
    fn parses_json_array() {
        let data = br#"[{"trace_id":"t1","span_id":"s1","service":"api","name":"request","start_ns":10,"end_ns":30}]"#;
        let spans = parse_spans_json(data).unwrap();
        assert_eq!(spans.len(), 1);
    }

    #[test]
    fn builds_index_and_assigns_lanes() {
        let spans = parse_spans_json(
            br#"[{"trace_id":"t1","span_id":"a","service":"api","name":"req","start_ns":0,"end_ns":20},{"trace_id":"t1","span_id":"b","service":"api","name":"db","start_ns":5,"end_ns":25},{"trace_id":"t1","span_id":"c","service":"api","name":"cache","start_ns":25,"end_ns":30}]"#,
        )
        .unwrap();
        let index = TraceIndex::build(spans).unwrap();
        let visible = index.query_window(0, 40, 10).unwrap();
        assert_eq!(visible.len(), 3);
        assert_eq!(visible[0].lane, 0);
        assert_eq!(visible[1].lane, 1);
        assert_eq!(visible[2].lane, 0);
        assert_eq!(index.track_summaries()[0].lanes, 2);
    }

    #[test]
    fn query_window_filters_and_limits() {
        let spans = parse_spans_json(
            br#"[{"trace_id":"t1","span_id":"a","service":"api","name":"req","start_ns":0,"end_ns":20},{"trace_id":"t1","span_id":"b","service":"db","name":"q","start_ns":50,"end_ns":70},{"trace_id":"t1","span_id":"c","service":"db","name":"q2","start_ns":55,"end_ns":80}]"#,
        )
        .unwrap();
        let index = TraceIndex::build(spans).unwrap();
        let visible = index.query_window(40, 60, 1).unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].span_id, "b");
    }

    #[test]
    fn reports_bounds() {
        let spans = parse_spans_json(
            br#"[{"trace_id":"t1","span_id":"a","service":"api","name":"req","start_ns":10,"end_ns":20},{"trace_id":"t2","span_id":"b","service":"db","name":"q","start_ns":30,"end_ns":90}]"#,
        )
        .unwrap();
        let index = TraceIndex::build(spans).unwrap();
        assert_eq!(index.time_bounds(), Some((10, 90)));
    }
}
