//! Incremental server-sent-events scanner. Observes bytes without changing
//! them; used to spot error events in model streams.

/// One dispatched event.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

impl SseEvent {
    /// The event name, or the JSON `type` of the data when no `event:` line
    /// was sent (OpenAI-style streams).
    pub fn kind(&self) -> Option<String> {
        if let Some(event) = &self.event {
            return Some(event.clone());
        }
        let value: serde_json::Value = serde_json::from_str(&self.data).ok()?;
        value.get("type")?.as_str().map(String::from)
    }

    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.data).ok()
    }
}

pub(crate) struct SseScanner {
    line: Vec<u8>,
    event: Option<String>,
    data: String,
    has_data: bool,
    /// The current line or event outgrew `max`; drop it.
    oversized: bool,
    max: usize,
}

impl SseScanner {
    pub fn new(max: usize) -> Self {
        Self {
            line: Vec::new(),
            event: None,
            data: String::new(),
            has_data: false,
            oversized: false,
            max,
        }
    }

    /// Feeds bytes; complete events are appended to `out`.
    pub fn push(&mut self, mut chunk: &[u8], out: &mut Vec<SseEvent>) {
        while !chunk.is_empty() {
            match memchr(b'\n', chunk) {
                Some(i) => {
                    if !self.oversized {
                        self.line.extend_from_slice(&chunk[..i]);
                    }
                    chunk = &chunk[i + 1..];
                    self.end_line(out);
                }
                None => {
                    if !self.oversized {
                        self.line.extend_from_slice(chunk);
                        if self.line.len() > self.max {
                            self.oversized = true;
                            self.line = Vec::new();
                        }
                    }
                    return;
                }
            }
        }
    }

    fn end_line(&mut self, out: &mut Vec<SseEvent>) {
        let mut line = std::mem::take(&mut self.line);
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            // Blank line: dispatch (an oversized event is dropped whole).
            if (self.has_data || self.event.is_some()) && !self.oversized {
                out.push(SseEvent {
                    event: self.event.take(),
                    data: std::mem::take(&mut self.data),
                });
            }
            self.reset_event();
            return;
        }
        if self.oversized {
            return;
        }
        let line = String::from_utf8_lossy(&line);
        let (field, value) = match line.split_once(':') {
            Some((field, value)) => (field, value.strip_prefix(' ').unwrap_or(value)),
            None => (&*line, ""),
        };
        match field {
            "event" => self.event = Some(value.to_string()),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
                if self.data.len() > self.max {
                    self.oversized = true;
                }
            }
            _ => {}
        }
    }

    fn reset_event(&mut self) {
        self.event = None;
        self.data.clear();
        self.has_data = false;
        self.oversized = false;
    }
}

fn memchr(needle: u8, haystack: &[u8]) -> Option<usize> {
    haystack.iter().position(|&b| b == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<SseEvent> {
        let mut scanner = SseScanner::new(1024);
        let mut out = Vec::new();
        for c in chunks {
            scanner.push(c, &mut out);
        }
        out
    }

    #[test]
    fn split_across_chunks() {
        let out = scan(&[
            b"event: pi",
            b"ng\ndata: {\"a\"",
            b":1}\r\n\r\n: comment\n\ndata: x\n",
            b"data: y\n\n",
        ]);
        assert_eq!(
            out,
            vec![
                SseEvent {
                    event: Some("ping".into()),
                    data: "{\"a\":1}".into()
                },
                SseEvent {
                    event: None,
                    data: "x\ny".into()
                },
            ]
        );
        assert_eq!(out[0].kind().as_deref(), Some("ping"));
    }

    #[test]
    fn kind_from_data_type() {
        let out = scan(&[b"data: {\"type\":\"response.created\"}\n\n"]);
        assert_eq!(out[0].kind().as_deref(), Some("response.created"));
    }

    #[test]
    fn oversized_event_dropped() {
        let mut scanner = SseScanner::new(8);
        let mut out = Vec::new();
        scanner.push(b"data: 0123456789abcdef\n\ndata: ok\n\n", &mut out);
        assert_eq!(
            out,
            vec![SseEvent {
                event: None,
                data: "ok".into()
            }]
        );
    }
}
