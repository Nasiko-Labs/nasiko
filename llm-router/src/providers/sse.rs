//! Minimal Server-Sent-Events reader shared by the streaming spokes.
//!
//! Yields the `data:` payload of each SSE event from a raw byte stream. Works for any
//! `data:`-framed source — OpenAI, Anthropic, and Gemini (`alt=sse`) all use it; each
//! spoke parses the payload in its own format. `[DONE]` is yielded verbatim for the
//! caller to recognize.

use futures::{Stream, StreamExt};

use super::ProviderError;

/// Turn a byte stream into a stream of SSE `data:` payloads.
pub(crate) fn sse_data_stream<S, B>(
    byte_stream: S,
) -> impl Stream<Item = Result<String, ProviderError>>
where
    S: Stream<Item = reqwest::Result<B>>,
    B: AsRef<[u8]>,
{
    async_stream::stream! {
        futures::pin_mut!(byte_stream);
        let mut buf: Vec<u8> = Vec::new();
        while let Some(item) = byte_stream.next().await {
            match item {
                Err(e) => {
                    yield Err(ProviderError::Transport(e.to_string()));
                    return;
                }
                Ok(bytes) => {
                    buf.extend_from_slice(bytes.as_ref());
                    // Emit every complete event (terminated by a blank line).
                    while let Some(pos) = find_event_end(&buf) {
                        let event: Vec<u8> = buf.drain(..pos).collect();
                        buf.drain(..2); // consume the "\n\n"
                        if let Some(data) = extract_data(&event) {
                            yield Ok(data);
                        }
                    }
                }
            }
        }
        // Flush any trailing event without a final blank line.
        if let Some(data) = extract_data(&buf) {
            yield Ok(data);
        }
    }
}

fn find_event_end(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

/// Concatenate the `data:` lines of one SSE event (CRLF tolerated via `str::lines`).
fn extract_data(event: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(event);
    let mut data = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        }
    }
    (!data.is_empty()).then_some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes_stream(chunks: Vec<&'static str>) -> impl Stream<Item = reqwest::Result<&'static [u8]>> {
        futures::stream::iter(chunks.into_iter().map(|s| Ok(s.as_bytes())))
    }

    #[tokio::test]
    async fn splits_events_across_chunk_boundaries() {
        // The "\n\n" separating the two events is split across byte chunks.
        let s = sse_data_stream(bytes_stream(vec![
            "data: one\n",
            "\ndata: two\n\n",
            "data: [DONE]\n\n",
        ]));
        let got: Vec<String> = s.filter_map(|r| async { r.ok() }).collect().await;
        assert_eq!(got, vec!["one", "two", "[DONE]"]);
    }
}
