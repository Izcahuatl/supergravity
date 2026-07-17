/// Incremental UTF-8-safe line splitter. Feed string chunks; get back complete
/// lines without terminators. Handles `\n` and `\r\n`.
#[derive(Default)]
pub struct LineDecoder {
    buf: String,
}

impl LineDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(chunk);
        let mut lines = Vec::new();
        while let Some(pos) = self.buf.find('\n') {
            let mut line: String = self.buf.drain(..=pos).collect();
            line.pop(); // '\n'
            if line.ends_with('\r') {
                line.pop();
            }
            lines.push(line);
        }
        lines
    }

    /// Flush a trailing unterminated line, if any.
    pub fn finish(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buf))
        }
    }
}

/// One parsed Server-Sent-Events block.
#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

/// Incremental SSE parser built on [`LineDecoder`]. Events are dispatched on
/// blank lines; `:` comment lines and `id:`/`retry:` fields are ignored.
#[derive(Default)]
pub struct SseDecoder {
    lines: LineDecoder,
    cur_event: Option<String>,
    cur_data: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, chunk: &str) -> Vec<SseEvent> {
        let mut out = Vec::new();
        for line in self.lines.push(chunk) {
            self.process_line(line, &mut out);
        }
        out
    }

    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if let Some(line) = self.lines.finish() {
            self.process_line(line, &mut out);
        }
        self.dispatch(&mut out);
        out
    }

    fn process_line(&mut self, line: String, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            self.dispatch(out);
        } else if line.starts_with(':') {
            // comment / keepalive
        } else if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            self.cur_data.push(data.to_string());
        } else if let Some(ev) = line.strip_prefix("event:") {
            let ev = ev.strip_prefix(' ').unwrap_or(ev);
            self.cur_event = Some(ev.to_string());
        }
        // id: and retry: fields are ignored
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) {
        if self.cur_data.is_empty() && self.cur_event.is_none() {
            return;
        }
        out.push(SseEvent {
            event: self.cur_event.take(),
            data: std::mem::take(&mut self.cur_data).join("\n"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_decoder_splits_chunks() {
        let mut d = LineDecoder::new();
        assert!(d.push("hel").is_empty());
        assert_eq!(d.push("lo\nwor"), vec!["hello".to_string()]);
        assert_eq!(d.push("ld\n"), vec!["world".to_string()]);
        assert_eq!(d.finish(), None);
    }

    #[test]
    fn line_decoder_crlf_and_trailing() {
        let mut d = LineDecoder::new();
        assert_eq!(d.push("a\r\nb\r\n"), vec!["a".to_string(), "b".to_string()]);
        let mut d2 = LineDecoder::new();
        assert!(d2.push("tail").is_empty());
        assert_eq!(d2.finish(), Some("tail".to_string()));
    }

    #[test]
    fn sse_single_data_event() {
        let mut d = SseDecoder::new();
        let evs = d.push("data: {\"a\":1}\n\n");
        assert_eq!(
            evs,
            vec![SseEvent {
                event: None,
                data: "{\"a\":1}".to_string()
            }]
        );
    }

    #[test]
    fn sse_multiline_data_and_event_field() {
        let mut d = SseDecoder::new();
        let evs = d.push("event: message_start\ndata: {\"x\":\ndata: 1}\n\n");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].event.as_deref(), Some("message_start"));
        assert_eq!(evs[0].data, "{\"x\":\n1}");
    }

    #[test]
    fn sse_comments_and_empty_lines_ignored() {
        let mut d = SseDecoder::new();
        let evs = d.push(": keepalive\n\n\ndata: hi\n\n");
        assert_eq!(
            evs,
            vec![SseEvent {
                event: None,
                data: "hi".to_string()
            }]
        );
    }

    #[test]
    fn sse_chunk_split_across_events() {
        let mut d = SseDecoder::new();
        assert!(d.push("data: one\n").is_empty());
        let evs = d.push("\ndata: two\n\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].data, "one");
        assert_eq!(evs[1].data, "two");
    }

    #[test]
    fn sse_finish_flushes_pending_event() {
        let mut d = SseDecoder::new();
        assert!(d.push("data: last\n").is_empty());
        let evs = d.finish();
        assert_eq!(
            evs,
            vec![SseEvent {
                event: None,
                data: "last".to_string()
            }]
        );
    }
}
