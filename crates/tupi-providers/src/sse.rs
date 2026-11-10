use tupi_kernel::ProviderError;

#[derive(Default)]
pub struct SseParser {
    buffer: Vec<u8>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(bytes);
        let mut payloads = Vec::new();

        while let Some((end, separator)) = find_event_end(&self.buffer) {
            let block: Vec<u8> = self.buffer.drain(..end + separator).collect();

            for line in block[..end].split(|byte| *byte == b'\n') {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                let Some(data) = line.strip_prefix(b"data:") else {
                    continue;
                };
                let data = data.strip_prefix(b" ").unwrap_or(data);
                if data.is_empty() {
                    continue;
                }
                if let Ok(data) = std::str::from_utf8(data) {
                    payloads.push(data.to_string());
                }
            }
        }

        payloads
    }
}

fn find_event_end(buffer: &[u8]) -> Option<(usize, usize)> {
    for index in 0..buffer.len() {
        if buffer[index] != b'\n' {
            continue;
        }
        if buffer.get(index + 1) == Some(&b'\n') {
            return Some((index, 2));
        }
        if buffer.get(index + 1) == Some(&b'\r') && buffer.get(index + 2) == Some(&b'\n') {
            return Some((index, 3));
        }
    }
    None
}

pub fn extract_delta(payload: &str) -> Result<Option<String>, ProviderError> {
    let value: serde_json::Value = serde_json::from_str(payload)
        .map_err(|error| ProviderError::Protocol(format!("JSON inválido no SSE: {error}")))?;

    Ok(value
        .pointer("/choices/0/delta/content")
        .and_then(|content| content.as_str())
        .map(str::to_string))
}

pub fn is_done(payload: &str) -> bool {
    payload.trim() == "[DONE]"
}

#[cfg(test)]
mod tests {
    use super::*;
    use tupi_kernel::ProviderError;

    #[test]
    fn handles_line_and_utf8_split_across_chunks() {
        let mut parser = SseParser::new();
        let text = "data: {\"choices\":[{\"delta\":{\"content\":\"olá\"}}]}\n\n";
        let bytes = text.as_bytes();
        let cut = text.find("á").unwrap() + 1;

        assert!(parser.push(&bytes[..cut]).is_empty());
        let payloads = parser.push(&bytes[cut..]);

        assert_eq!(payloads.len(), 1);
        assert_eq!(extract_delta(&payloads[0]).unwrap(), Some("olá".into()));
    }

    #[test]
    fn ignores_comments_and_done() {
        let mut parser = SseParser::new();

        assert!(parser.push(b": keep-alive\n\n").is_empty());
        let payloads = parser.push(b"data: [DONE]\n\n");

        assert_eq!(payloads.len(), 1);
        assert!(is_done(&payloads[0]));
    }

    #[test]
    fn emits_multiple_events_from_one_chunk() {
        let mut parser = SseParser::new();
        let text = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n\n"
        );

        let payloads = parser.push(text.as_bytes());

        assert_eq!(payloads.len(), 2);
        assert_eq!(extract_delta(&payloads[1]).unwrap(), Some("b".into()));
    }

    #[test]
    fn invalid_json_is_protocol_error() {
        let error = extract_delta("{\"choices\":").unwrap_err();
        assert!(matches!(error, ProviderError::Protocol(_)));
    }
}
