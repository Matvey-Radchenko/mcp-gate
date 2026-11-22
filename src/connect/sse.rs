//! Incremental SSE framing. A chunk can end anywhere in a line or code point.
pub(super) struct Parser {
    line: Vec<u8>,
    data: Vec<u8>,
    first_line: bool,
    skip_lf: bool,
}

impl Parser {
    pub(super) fn new() -> Self {
        Self {
            line: Vec::new(),
            data: Vec::new(),
            first_line: true,
            skip_lf: false,
        }
    }

    // Only a blank line dispatches an event. A partial event at EOF is dropped.
    pub(super) fn next(&mut self, chunk: &mut &[u8]) -> Option<Vec<u8>> {
        while let Some((&byte, tail)) = chunk.split_first() {
            *chunk = tail;
            if std::mem::take(&mut self.skip_lf) && byte == b'\n' {
                continue;
            }
            if matches!(byte, b'\r' | b'\n') {
                self.skip_lf = byte == b'\r';
                let event = self.finish_line();
                self.line.clear();
                if event.is_some() {
                    return event;
                }
            } else {
                self.line.push(byte);
            }
        }
        None
    }

    fn finish_line(&mut self) -> Option<Vec<u8>> {
        let mut line = self.line.as_slice();
        if std::mem::take(&mut self.first_line) {
            line = line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(line);
        }
        if line.is_empty() {
            // Stream priming and heartbeats can contain an empty data field.
            let event = (!self.data.iter().all(u8::is_ascii_whitespace))
                .then(|| std::mem::take(&mut self.data));
            self.data.clear();
            return event;
        }
        let mut parts = line.splitn(2, |byte| *byte == b':');
        if parts.next() == Some(b"data") {
            let value = parts.next().unwrap_or_default();
            self.data.extend(value.strip_prefix(b" ").unwrap_or(value));
            self.data.push(b'\n');
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sse_handles_chunk_boundaries_multiline_data_and_control_events() {
        let stream = "\u{feff}: keepalive\r\nid: 1\r\nretry: 1000\r\ndata:\r\n\r\n\
            data: {\"jsonrpc\":\"2.0\",\rdata: \"result\":\"café\",\"id\":1}\r\r\
            data:{\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n\n\
            data: {\"jsonrpc\":\"2.0\",\"method\":\"incomplete\"}\n";
        let mut parser = Parser::new();
        let actual: Vec<_> = stream
            .as_bytes()
            .chunks(1)
            .filter_map(|mut chunk| parser.next(&mut chunk))
            .map(|data| serde_json::from_slice::<serde_json::Value>(&data).unwrap())
            .collect();
        assert_eq!(
            actual,
            [
                json!({"jsonrpc":"2.0","result":"café","id":1}),
                json!({"jsonrpc":"2.0","method":"ping"})
            ]
        );
    }
}
