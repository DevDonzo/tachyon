use serde::Deserialize;
use tachyon_core::{Result, Span, TachyonError};

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
    fn filters_spans_in_window() {
        let spans = parse_spans_json(
            br#"[{"trace_id":"t1","span_id":"s1","service":"api","name":"request","start_ns":10,"end_ns":30},{"trace_id":"t1","span_id":"s2","service":"db","name":"query","start_ns":31,"end_ns":50}]"#,
        )
        .unwrap();
        let visible = spans_in_window(&spans, 15, 30).unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].span_id, "s1");
    }
}
