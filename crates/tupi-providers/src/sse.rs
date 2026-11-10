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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCallDelta {
    pub index: usize,
    pub id: Option<String>,
    pub name: Option<String>,
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamChunk {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCallDelta>,
    pub finish_reason: Option<String>,
    pub usage: Option<u64>,
}

pub fn extract_chunk(payload: &str) -> Result<StreamChunk, ProviderError> {
    let value: serde_json::Value = serde_json::from_str(payload)
        .map_err(|error| ProviderError::Protocol(format!("JSON inválido no SSE: {error}")))?;

    let text = value
        .pointer("/choices/0/delta/content")
        .and_then(|content| content.as_str())
        .map(str::to_string);

    let finish_reason = value
        .pointer("/choices/0/finish_reason")
        .and_then(|reason| reason.as_str())
        .map(str::to_string);

    let usage = value
        .pointer("/usage/completion_tokens")
        .and_then(|tokens| tokens.as_u64());

    let mut tool_calls = Vec::new();
    if let Some(calls) = value
        .pointer("/choices/0/delta/tool_calls")
        .and_then(|calls| calls.as_array())
    {
        for call in calls {
            tool_calls.push(ToolCallDelta {
                index: call
                    .get("index")
                    .and_then(|index| index.as_u64())
                    .unwrap_or(0) as usize,
                id: call
                    .get("id")
                    .and_then(|id| id.as_str())
                    .map(str::to_string),
                name: call
                    .pointer("/function/name")
                    .and_then(|name| name.as_str())
                    .map(str::to_string),
                arguments: call
                    .pointer("/function/arguments")
                    .and_then(|arguments| arguments.as_str())
                    .map(str::to_string),
            });
        }
    }

    Ok(StreamChunk {
        text,
        tool_calls,
        finish_reason,
        usage,
    })
}

pub fn extract_delta(payload: &str) -> Result<Option<String>, ProviderError> {
    Ok(extract_chunk(payload)?.text)
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

    #[test]
    fn chunk_reads_text_and_finish_reason() {
        let payload = r#"{"choices":[{"delta":{"content":"oi"},"finish_reason":"stop"}]}"#;
        let chunk = extract_chunk(payload).unwrap();

        assert_eq!(chunk.text.as_deref(), Some("oi"));
        assert_eq!(chunk.finish_reason.as_deref(), Some("stop"));
        assert!(chunk.tool_calls.is_empty());
    }

    #[test]
    fn chunk_reads_tool_call_fragments() {
        let payload = r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"read","arguments":"{\"path\""}}]},"finish_reason":null}]}"#;
        let chunk = extract_chunk(payload).unwrap();

        assert_eq!(chunk.tool_calls.len(), 1);
        let call = &chunk.tool_calls[0];
        assert_eq!(call.index, 0);
        assert_eq!(call.id.as_deref(), Some("c1"));
        assert_eq!(call.name.as_deref(), Some("read"));
        assert_eq!(call.arguments.as_deref(), Some("{\"path\""));
    }

    #[test]
    fn chunk_reads_usage_when_present() {
        let payload = r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":12,"total_tokens":19}}"#;
        let chunk = extract_chunk(payload).unwrap();

        assert_eq!(chunk.usage, Some(12));
    }

    #[test]
    fn chunk_without_usage_is_none() {
        let payload = r#"{"choices":[{"delta":{"content":"oi"}}],"usage":null}"#;
        let chunk = extract_chunk(payload).unwrap();

        assert_eq!(chunk.usage, None);
    }
}
