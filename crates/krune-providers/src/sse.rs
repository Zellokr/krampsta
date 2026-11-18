//! An incremental parser for Server-Sent Events (`text/event-stream`).
//!
//! Network chunks split events (and even UTF-8 characters) anywhere, so the parser keeps the
//! unfinished line's bytes and only decodes whole lines. It follows the WHATWG rules that
//! matter for model APIs: `\n`, `\r\n` or `\r` end a line, a blank line dispatches the event,
//! several `data:` lines join with `\n`, and lines starting with `:` are comments.

/// One dispatched event.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseEvent {
    /// The `event:` field; empty means the default type, `message`.
    pub event: String,
    /// The `data:` lines, joined with `\n`.
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseParser {
    /// Bytes of the line not yet terminated.
    line: Vec<u8>,
    /// The previous chunk ended on `\r`: a leading `\n` in the next one belongs to it.
    skip_lf: bool,
    event: String,
    data: String,
    has_data: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a chunk of the body; returns the events it completed, in order.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        for &b in chunk {
            if std::mem::take(&mut self.skip_lf) && b == b'\n' {
                continue;
            }
            match b {
                b'\n' | b'\r' => {
                    self.skip_lf = b == b'\r';
                    let line = std::mem::take(&mut self.line);
                    if let Some(event) = self.line_done(&String::from_utf8_lossy(&line)) {
                        out.push(event);
                    }
                }
                _ => self.line.push(b),
            }
        }
        out
    }

    fn line_done(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            "event" => value.clone_into(&mut self.event),
            // `id` and `retry` only matter for reconnecting, which model streams never do.
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseEvent> {
        let event = std::mem::take(&mut self.event);
        if !std::mem::take(&mut self.has_data) {
            return None;
        }
        Some(SseEvent { event, data: std::mem::take(&mut self.data) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(events: &[SseEvent]) -> Vec<&str> {
        events.iter().map(|e| e.data.as_str()).collect()
    }

    #[test]
    fn parses_events_separated_by_blank_lines() {
        let mut p = SseParser::new();
        let events = p.push(b"data: one\n\ndata: two\n\n");
        assert_eq!(data(&events), ["one", "two"]);
    }

    #[test]
    fn events_split_across_chunks_arrive_whole() {
        let mut p = SseParser::new();
        assert!(p.push(b"da").is_empty());
        assert!(p.push(b"ta: hel").is_empty());
        assert!(p.push(b"lo\n").is_empty());
        assert_eq!(data(&p.push(b"\n")), ["hello"]);
    }

    #[test]
    fn utf8_split_mid_character_is_decoded_once_whole() {
        let text = "data: ñandú\n\n".as_bytes();
        let mut p = SseParser::new();
        let mut events = Vec::new();
        for b in text {
            events.extend(p.push(std::slice::from_ref(b)));
        }
        assert_eq!(data(&events), ["ñandú"]);
    }

    #[test]
    fn crlf_and_cr_line_endings_work_even_split() {
        let mut p = SseParser::new();
        let mut events = p.push(b"data: a\r");
        events.extend(p.push(b"\n\r\n"));
        events.extend(p.push(b"data: b\r\r"));
        assert_eq!(data(&events), ["a", "b"]);
    }

    #[test]
    fn multiline_data_comments_and_event_names() {
        let mut p = SseParser::new();
        let events = p.push(b": keep-alive\nevent: delta\ndata: x\ndata:y\nid: 3\n\n");
        assert_eq!(events, [SseEvent { event: "delta".into(), data: "x\ny".into() }]);
    }

    #[test]
    fn blank_lines_without_data_dispatch_nothing() {
        let mut p = SseParser::new();
        assert!(p.push(b"\n\nevent: ping\n\n").is_empty());
        // The ignored event's name does not leak into the next one.
        assert_eq!(p.push(b"data: z\n\n")[0].event, "");
    }
}
