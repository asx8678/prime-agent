//! Server-sent event decoding shared by streaming providers.
//!
//! Mirrors the TS SSE decoder: \n, \r\n and \r line breaks, comment lines, `event:`/`data:` fields
//! and multi-line data, blank-line/EOF flush. A `\r\n` split across chunk
//! boundaries counts as one line break, wherever the transport splits it.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSentEvent {
    pub event: Option<String>,
    pub data: String,
    pub raw: Vec<String>,
}

#[derive(Debug, Default)]
struct DecoderState {
    event: Option<String>,
    data: Vec<String>,
    raw: Vec<String>,
}

impl DecoderState {
    fn flush(&mut self) -> Option<ServerSentEvent> {
        if self.event.is_none() && self.data.is_empty() {
            return None;
        }
        let event = ServerSentEvent {
            event: self.event.take(),
            data: std::mem::take(&mut self.data).join("\n"),
            raw: std::mem::take(&mut self.raw),
        };
        Some(event)
    }

    fn decode_line(&mut self, line: &str) -> Option<ServerSentEvent> {
        if line.is_empty() {
            return self.flush();
        }
        self.raw.push(line.to_string());
        if line.starts_with(':') {
            return None;
        }
        let (field_name, value) = match line.find(':') {
            Some(index) => {
                let name = &line[..index];
                let mut value = &line[index + 1..];
                if let Some(stripped) = value.strip_prefix(' ') {
                    value = stripped;
                }
                (name, value)
            }
            None => (line, ""),
        };
        if field_name == "event" {
            self.event = Some(value.to_string());
        } else if field_name == "data" {
            self.data.push(value.to_string());
        }
        None
    }
}

/// Incremental SSE decoder over text chunks.
#[derive(Debug, Default)]
pub struct SseDecoder {
    state: DecoderState,
    buffer: String,
    /// The previous chunk ended in a lone `\r`: a leading `\n` in the next
    /// chunk is the second half of that `\r\n`, not another line break.
    pending_cr: bool,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk of decoded text; returns complete events in order.
    pub fn push_text(&mut self, text: &str) -> Vec<ServerSentEvent> {
        let mut index = self.buffer.len();
        self.buffer.push_str(text);
        let mut events = Vec::new();
        let mut line_start = 0;
        let bytes = self.buffer.as_bytes();
        // A `\r\n` split across the chunk boundary is one line break: the
        // trailing `\r` already ended the previous line (the buffer is
        // empty when the flag is set), so a leading `\n` here is consumed
        // as its second half.
        if self.pending_cr && !text.is_empty() {
            self.pending_cr = false;
            if bytes[0] == b'\n' {
                index = 1;
                line_start = 1;
            }
        }
        while index < bytes.len() {
            let break_len = match bytes[index] {
                b'\r' if bytes.get(index + 1) == Some(&b'\n') => 2,
                b'\r' | b'\n' => 1,
                _ => {
                    index += 1;
                    continue;
                }
            };
            if let Some(event) = self.state.decode_line(&self.buffer[line_start..index]) {
                events.push(event);
            }
            // A lone `\r` as the chunk's final byte may be the first half
            // of a `\r\n` split across the boundary.
            if bytes[index] == b'\r' && break_len == 1 && index + 1 == bytes.len() {
                self.pending_cr = true;
            }
            index += break_len;
            line_start = index;
        }
        self.buffer.drain(..line_start);
        events
    }

    /// Signal end of stream; flushes the final line and trailing event.
    pub fn finish(&mut self) -> Vec<ServerSentEvent> {
        let mut events = Vec::new();
        // Remaining buffer without a line break is the last line.
        if !self.buffer.is_empty() {
            let line = std::mem::take(&mut self.buffer);
            if let Some(event) = self.state.decode_line(&line) {
                events.push(event);
            }
        }
        if let Some(event) = self.state.flush() {
            events.push(event);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_basic_events() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push_text("event: message_start\ndata: {\"a\":1}\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event.as_deref(), Some("message_start"));
        assert_eq!(events[0].data, "{\"a\":1}");
    }

    #[test]
    fn handles_crlf_and_multiline_data() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push_text("data: line1\r\ndata: line2\r\n\r\n");
        assert_eq!(events[0].data, "line1\nline2");
        assert_eq!(events[0].event, None);
    }

    #[test]
    fn skips_comments_and_unknown_fields() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push_text(": keep-alive\ndata: x\nid: 3\n\n");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "x");
    }

    #[test]
    fn decodes_prime_inference_capture() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/testdata/prime_inference_glm53_flash.sse"
        ))
        .unwrap();
        let mut decoder = SseDecoder::new();
        let events = decoder.push_text(&text);
        let extra = decoder.finish();
        assert_eq!(events.len() + extra.len(), 44); // 43 chunks + [DONE]
        let expected_lines: Vec<&str> = text
            .lines()
            .filter(|line| line.starts_with("data: "))
            .map(|line| &line[6..])
            .collect();
        let actual: Vec<&str> = events
            .iter()
            .chain(extra.iter())
            .map(|event| event.data.as_str())
            .collect();
        assert_eq!(actual, expected_lines);
        let empty: Vec<&str> = events
            .iter()
            .chain(extra.iter())
            .filter(|e| e.data.is_empty())
            .map(|_| "empty")
            .collect();
        assert!(empty.is_empty(), "found empty data events");
        assert!(events[0].data.contains("chat.completion.chunk"));
    }

    #[test]
    fn preserves_fragmented_line_breaks_and_raw_lines() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push_text("data: first\r").is_empty());
        // The split CRLF completes the line; no event until a blank line.
        assert!(decoder.push_text("\n").is_empty());
        assert_eq!(
            decoder.push_text("event: update\rdata: next\n\n"),
            vec![ServerSentEvent {
                event: Some("update".into()),
                data: "first\nnext".into(),
                raw: vec![
                    "data: first".into(),
                    "event: update".into(),
                    "data: next".into()
                ],
            }]
        );
        assert!(decoder.push_text("data: final").is_empty());
        assert_eq!(
            decoder.finish(),
            vec![ServerSentEvent {
                event: None,
                data: "final".into(),
                raw: vec!["data: final".into()],
            }]
        );
    }

    #[test]
    fn crlf_split_across_chunks_is_one_line_break() {
        // A `\r` ending one chunk and the `\n` opening the next are one
        // `\r\n`: the stream must decode identically to the unsplit bytes
        // ("data: a\r\ndata: b\r\n\r\n" → one event, "a\nb").
        let mut decoder = SseDecoder::new();
        assert!(decoder.push_text("data: a\r").is_empty());
        assert!(decoder.push_text("\ndata: b\r").is_empty());
        assert_eq!(
            decoder.push_text("\n\r\n"),
            vec![ServerSentEvent {
                event: None,
                data: "a\nb".into(),
                raw: vec!["data: a".into(), "data: b".into()],
            }]
        );
    }

    #[test]
    fn flushes_trailing_event_at_eof() {
        let mut decoder = SseDecoder::new();
        decoder.push_text("data: partial");
        let events = decoder.finish();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].data, "partial");
    }
}
