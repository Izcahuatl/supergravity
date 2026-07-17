use crate::core::error::{Error, Result};
use futures::{Stream, StreamExt};

/// Incremental UTF-8 decoder for raw byte chunks. Multi-byte characters split
/// across chunk boundaries are held back until complete (a plain
/// `String::from_utf8_lossy` per chunk would corrupt them into U+FFFD).
pub(crate) struct Utf8Buf {
    buf: Vec<u8>,
}

impl Utf8Buf {
    pub(crate) fn new() -> Self {
        Utf8Buf { buf: Vec::new() }
    }

    /// Feed raw bytes. Returns the newly decodable text, if any.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Option<String> {
        self.buf.extend_from_slice(chunk);
        let (emit, drain) = match std::str::from_utf8(&self.buf) {
            Ok(_) => (String::from_utf8_lossy(&self.buf).into_owned(), self.buf.len()),
            Err(e) => {
                let valid = e.valid_up_to();
                if e.error_len().is_some() {
                    // genuinely invalid byte(s): lossy-emit everything, start over
                    (String::from_utf8_lossy(&self.buf).into_owned(), self.buf.len())
                } else {
                    // incomplete trailing sequence: emit the valid prefix, keep the rest
                    (String::from_utf8_lossy(&self.buf[..valid]).into_owned(), valid)
                }
            }
        };
        self.buf.drain(..drain);
        if emit.is_empty() {
            None
        } else {
            Some(emit)
        }
    }

    /// Flush at end of stream; an incomplete trailing sequence is lossy-emitted.
    pub(crate) fn finish(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            None
        } else {
            let s = String::from_utf8_lossy(&self.buf).into_owned();
            self.buf.clear();
            Some(s)
        }
    }
}

/// POST a request and return the response body as a stream of text chunks.
/// Non-2xx responses become [`Error::Provider`] with a truncated body.
/// Per-request timeout: 120 s.
pub async fn post_stream(
    req: reqwest::RequestBuilder,
) -> Result<impl Stream<Item = Result<String>> + Send> {
    let resp = req.timeout(std::time::Duration::from_secs(120)).send().await?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let body: String = body.chars().take(500).collect();
        return Err(Error::Provider { status: status.as_u16(), body });
    }
    let bytes = resp.bytes_stream();
    let stream = async_stream::try_stream! {
        let mut utf8 = Utf8Buf::new();
        tokio::pin!(bytes);
        while let Some(chunk) = bytes.next().await {
            let chunk = chunk?;
            if let Some(text) = utf8.push(&chunk) {
                yield text;
            }
        }
        if let Some(text) = utf8.finish() {
            yield text;
        }
    };
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8buf_complete_chunk() {
        let mut b = Utf8Buf::new();
        assert_eq!(b.push("héllo".as_bytes()), Some("héllo".to_string()));
        assert_eq!(b.finish(), None);
    }

    #[test]
    fn utf8buf_split_multibyte_char_across_chunks() {
        // 'é' is 0xC3 0xA9 — split between the two bytes.
        let mut b = Utf8Buf::new();
        assert_eq!(b.push(&[0x68, 0xC3]), Some("h".to_string()));
        assert_eq!(b.push(&[0xA9, 0x21]), Some("é!".to_string()));
    }

    #[test]
    fn utf8buf_invalid_byte_is_replaced() {
        let mut b = Utf8Buf::new();
        let out = b.push(&[0xFF]).unwrap();
        assert!(out.contains('\u{FFFD}'), "{out}");
    }

    #[test]
    fn utf8buf_finish_flushes_incomplete_tail_lossy() {
        let mut b = Utf8Buf::new();
        assert_eq!(b.push(&[0x68]), Some("h".to_string()));
        let _ = b.push(&[0xC3]);
        let tail = b.finish().unwrap();
        assert!(tail.contains('\u{FFFD}'), "{tail}");
    }
}
