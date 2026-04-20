use tachyon_core::Span;

pub fn synthetic_log_bytes(lines: usize, payload_len: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(lines * (payload_len + 32));
    for idx in 0..lines {
        let line = format!(
            "2026-01-01T00:{:02}:{:02}Z service=api level=INFO req={:08x} payload={:0payload_len$}\n",
            (idx / 60) % 60,
            idx % 60,
            idx,
            "",
            payload_len = payload_len
        );
        bytes.extend_from_slice(line.as_bytes());
    }
    bytes
}

pub fn synthetic_trace_spans(service_count: usize, spans_per_service: usize) -> Vec<Span> {
    let mut spans = Vec::with_capacity(service_count * spans_per_service);
    for service_idx in 0..service_count {
        let service = format!("svc-{service_idx:03}");
        for span_idx in 0..spans_per_service {
            let start_ns = (span_idx as u64 * 10_000) + ((service_idx as u64 % 7) * 500);
            let duration_ns = 5_000 + ((span_idx as u64 % 11) * 350);
            spans.push(Span {
                trace_id: format!("trace-{:04}", span_idx / 8),
                span_id: format!("{service_idx:03}-{span_idx:06}"),
                parent_span_id: (span_idx > 0)
                    .then(|| format!("{service_idx:03}-{:06}", span_idx - 1)),
                service: service.clone(),
                name: format!("op-{}", span_idx % 16),
                start_ns,
                end_ns: start_ns + duration_ns,
            });
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_emits_requested_number_of_lines() {
        let bytes = synthetic_log_bytes(3, 8);
        assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 3);
    }

    #[test]
    fn synthetic_trace_helper_emits_expected_count() {
        let spans = synthetic_trace_spans(4, 25);
        assert_eq!(spans.len(), 100);
    }
}
