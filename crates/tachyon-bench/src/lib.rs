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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generator_emits_requested_number_of_lines() {
        let bytes = synthetic_log_bytes(3, 8);
        assert_eq!(bytes.iter().filter(|byte| **byte == b'\n').count(), 3);
    }
}
