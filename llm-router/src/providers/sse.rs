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
                    while let Some((pos, sep_len)) = find_event_end(&buf) {
                        let event: Vec<u8> = buf.drain(..pos).collect();
                        buf.drain(..sep_len); // consume the blank-line separator
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

/// Blank-line event separators the SSE spec allows. Gemini uses `\r\n\r\n`. The CRLF form
/// comes before `\r\r` so a CRLF pair is never read as two bare CRs.
const EVENT_SEPARATORS: [&[u8]; 3] = [b"\r\n\r\n", b"\n\n", b"\r\r"];

/// Position and length of the first event separator in `buf`, if a complete one is there.
fn find_event_end(buf: &[u8]) -> Option<(usize, usize)> {
    (0..buf.len()).find_map(|pos| {
        EVENT_SEPARATORS
            .iter()
            .find(|sep| buf[pos..].starts_with(sep))
            .map(|sep| (pos, sep.len()))
    })
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

    fn bytes_stream(
        chunks: Vec<&'static str>,
    ) -> impl Stream<Item = reqwest::Result<&'static [u8]>> {
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

    #[tokio::test]
    async fn splits_crlf_events_across_chunk_boundaries() {
        // Gemini ends events with "\r\n\r\n"; here the separator is split across chunks.
        let s = sse_data_stream(bytes_stream(vec![
            "data: one\r\n\r",
            "\ndata: two\r\n",
            "\r\ndata: [DONE]\r\n\r\n",
        ]));
        let got: Vec<String> = s.filter_map(|r| async { r.ok() }).collect().await;
        assert_eq!(got, vec!["one", "two", "[DONE]"]);
    }

    /// The two events Gemini really streamed for QA test PR-01 (gemini-3.1-flash-lite), framed with
    /// Google's `\r\n\r\n`. The second has empty text plus a `thoughtSignature`.
    const GEMINI_PR01_STREAM: &str = concat!(
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"42\"}],\"role\":\"model\"},\"index\":0}],",
        "\"usageMetadata\":{\"promptTokenCount\":16,\"candidatesTokenCount\":2,\"totalTokenCount\":18,",
        "\"promptTokensDetails\":[{\"modality\":\"TEXT\",\"tokenCount\":16}],\"serviceTier\":\"standard\"},",
        "\"modelVersion\":\"gemini-3.1-flash-lite\",\"responseId\":\"OV30arqIA52v694PgqiioAY\"}\r\n\r\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"\",\"thoughtSignature\":",
        "\"EnMKcQFpFH0T25PNK1ycdkqkRkLEbe5/dO/kNe1hnw5T0DdoeZ7aPDNfq9Teg6+FSRToPSEtNwuGkggv6OTYFbTLS5i6T55XXwLJUQ5jfC/ljM2H3JuM4NaYPXzSMkF/F443CpC0Wuqm8pRvRBBQilRQGMuy\"}],",
        "\"role\":\"model\"},\"finishReason\":\"STOP\",\"index\":0}],",
        "\"usageMetadata\":{\"promptTokenCount\":16,\"candidatesTokenCount\":2,\"totalTokenCount\":18,",
        "\"promptTokensDetails\":[{\"modality\":\"TEXT\",\"tokenCount\":16}],\"serviceTier\":\"standard\"},",
        "\"modelVersion\":\"gemini-3.1-flash-lite\",\"responseId\":\"OV30arqIA52v694PgqiioAY\"}\r\n\r\n",
    );

    #[tokio::test]
    async fn reads_a_real_gemini_crlf_stream() {
        let bytes = GEMINI_PR01_STREAM.as_bytes();
        let first_end = GEMINI_PR01_STREAM.find("\r\n\r\n").unwrap();
        // Cut inside the first "\r\n\r\n", then mid-way through the second event.
        let cuts = [0, 50, first_end + 2, first_end + 300, bytes.len()];
        let chunks: Vec<reqwest::Result<&[u8]>> =
            cuts.windows(2).map(|w| Ok(&bytes[w[0]..w[1]])).collect();
        let got: Vec<String> = sse_data_stream(futures::stream::iter(chunks))
            .filter_map(|r| async { r.ok() })
            .collect()
            .await;
        assert_eq!(got.len(), 2);
        for payload in &got {
            serde_json::from_str::<serde_json::Value>(payload).expect("payload is JSON");
        }
    }

    #[test]
    fn finds_each_separator_kind() {
        assert_eq!(find_event_end(b"data: a\n\nrest"), Some((7, 2)));
        assert_eq!(find_event_end(b"data: a\r\n\r\nrest"), Some((7, 4)));
        assert_eq!(find_event_end(b"data: a\r\rrest"), Some((7, 2)));
        assert_eq!(find_event_end(b"data: a\r\n\r"), None);
    }
}
