use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures::{StreamExt, TryStreamExt, stream};
use serde_json::{Map, Value, json};

use super::cache::AttachmentCache;
use super::marker::{
    AttachmentRef, MarkerClaims, MarkerSigner, TOKEN_PREFIX, human_size, shown_above_line,
    token_spans,
};
use super::store::{AttachmentFetchError, AttachmentStore};
use crate::error::GatewayError;
use crate::ir::{ChatRequest, Message};
use crate::providers::media::is_text_like;

const CURRENT_MESSAGE: &str = "\n\nCurrent message: ";
const FETCH_CONCURRENCY: usize = 4;
const PROBE_BYTES: usize = 512;
const PROBE_READ_BYTES: usize = PROBE_BYTES + UTF8_BOM.len() + 3;
const MIN_PROBE_BYTES: usize = 32;
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ExpandReport {
    pub attached: usize,
    pub skipped_media_present: usize,
    pub skipped_text_present: usize,
    pub repeated: usize,
    pub invalid: usize,
    pub bytes: u64,
    pub cache_hits: usize,
}

pub async fn expand(
    req: &mut ChatRequest,
    flow_id: &str,
    now: i64,
    signer: &MarkerSigner,
    store: &dyn AttachmentStore,
    cache: &AttachmentCache,
    max_total_bytes: u64,
) -> Result<ExpandReport, GatewayError> {
    let mut report = ExpandReport::default();
    let turn = current_turn(&req.messages);
    let instructions: Vec<usize> = (0..req.messages.len())
        .filter(|&i| is_instruction(&req.messages[i]))
        .collect();
    if !turn
        .iter()
        .chain(&instructions)
        .any(|&i| mentions_token(&req.messages[i]))
    {
        return Ok(report);
    }
    let host = turn.first().copied();
    let host_has_media = host.is_some_and(|i| has_media(&req.messages[i]));

    let mut files: Vec<MarkerClaims> = Vec::new();
    let mut slots: HashMap<AttachmentRef, usize> = HashMap::new();
    let mut hosted: Vec<usize> = Vec::new();
    let mut edits: Vec<(usize, Vec<TextEdit>)> = Vec::new();
    for &index in turn.iter().chain(&instructions) {
        let message = &req.messages[index];
        let instruction = is_instruction(message);
        let media_present = if instruction {
            host_has_media
        } else {
            has_media(message)
        };
        let mut texts = Vec::new();
        for (part, text, from) in scannable_texts(message) {
            let mut cuts = Vec::new();
            for span in token_spans(&text[from..]) {
                let token = &text[from + span.token.start..from + span.token.end];
                let Ok(claims) = signer.verify(token, flow_id, now) else {
                    report.invalid += 1;
                    continue;
                };
                let action = if media_present {
                    report.skipped_media_present += 1;
                    Action::Remove
                } else if let Some(&slot) = slots.get(&claims.reference) {
                    report.repeated += 1;
                    if instruction {
                        Action::Remove
                    } else {
                        Action::Repeat(slot)
                    }
                } else {
                    let slot = files.len();
                    slots.insert(claims.reference.clone(), slot);
                    files.push(claims);
                    if instruction {
                        hosted.push(slot);
                        Action::Remove
                    } else {
                        Action::Attach(slot)
                    }
                };
                cuts.push(Cut {
                    line: from + span.line.start..from + span.line.end,
                    action,
                });
            }
            if !cuts.is_empty() {
                texts.push(TextEdit { part, cuts });
            }
        }
        if !texts.is_empty() {
            edits.push((index, texts));
        }
    }

    let total: u64 = files
        .iter()
        .map(|f| f.size_bytes)
        .fold(0, u64::saturating_add);
    if total > max_total_bytes {
        return Err(GatewayError::BadRequest(format!(
            "the attached files total {}, over the {} limit for one model call",
            human_size(total),
            human_size(max_total_bytes)
        )));
    }

    let fetches: Vec<_> = files
        .iter()
        .map(|file| fetch_one(file, store, cache))
        .collect();
    let fetched: Vec<(Arc<str>, bool)> = stream::iter(fetches)
        .buffered(FETCH_CONCURRENCY)
        .try_collect()
        .await?;

    let pasted: Vec<bool> = files
        .iter()
        .zip(&fetched)
        .map(|(file, (base64, _))| already_pasted(&req.messages, file, base64))
        .collect();
    report.skipped_text_present = pasted.iter().filter(|&&p| p).count();
    report.attached = files.len() - report.skipped_text_present;
    report.bytes = files
        .iter()
        .zip(&pasted)
        .filter(|(_, pasted)| !**pasted)
        .map(|(file, _)| file.size_bytes)
        .fold(0, u64::saturating_add);
    report.cache_hits = fetched.iter().filter(|(_, hit)| *hit).count();
    let mut media: Vec<Option<Value>> = files
        .iter()
        .zip(&fetched)
        .zip(&pasted)
        .map(|((file, (base64, _)), pasted)| (!pasted).then(|| media_part(file, base64)))
        .collect();
    drop(fetched);
    for (_, texts) in &mut edits {
        for cut in texts.iter_mut().flat_map(|t| &mut t.cuts) {
            if let Action::Attach(slot) | Action::Repeat(slot) = cut.action
                && pasted[slot]
            {
                cut.action = Action::Remove;
            }
        }
    }
    let mut emptied: Vec<usize> = Vec::new();
    for (index, texts) in edits {
        apply(&mut req.messages[index], texts, &files, &mut media);
        if is_instruction(&req.messages[index]) && is_blank(&req.messages[index]) {
            emptied.push(index);
        }
    }
    let hosted: Vec<Value> = hosted
        .into_iter()
        .filter_map(|slot| media[slot].take())
        .collect();
    if !hosted.is_empty() {
        attach_to_turn(&mut req.messages, host, hosted);
    }
    emptied.sort_unstable();
    for index in emptied.into_iter().rev() {
        req.messages.remove(index);
    }
    Ok(report)
}

fn is_instruction(message: &Message) -> bool {
    matches!(message.role.as_str(), "system" | "developer")
}

fn is_blank(message: &Message) -> bool {
    match &message.content {
        Some(Value::String(text)) => text.trim().is_empty(),
        Some(Value::Array(parts)) => parts.iter().all(|p| {
            p.get("type").and_then(Value::as_str) == Some("text")
                && p.get("text")
                    .and_then(Value::as_str)
                    .is_none_or(|t| t.trim().is_empty())
        }),
        _ => true,
    }
}

fn attach_to_turn(messages: &mut Vec<Message>, host: Option<usize>, media: Vec<Value>) {
    let Some(message) = host.and_then(|i| messages.get_mut(i)) else {
        messages.push(Message {
            role: "user".into(),
            content: Some(Value::Array(media)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        });
        return;
    };
    let mut parts = match message.content.take() {
        Some(Value::Array(parts)) => parts,
        Some(Value::String(text)) if !text.trim().is_empty() => {
            vec![json!({"type": "text", "text": text})]
        }
        _ => Vec::new(),
    };
    parts.extend(media);
    message.content = Some(Value::Array(parts));
}

pub(crate) async fn attach_marked_files(
    ctx: &crate::LlmRouterCtx,
    req: &mut ChatRequest,
    flow_id: Option<&str>,
    agent_id: &str,
) -> Result<(), GatewayError> {
    let (Some(flow_id), Some(signer)) = (flow_id, ctx.marker_signer.as_deref()) else {
        return Ok(());
    };
    let report = expand(
        req,
        flow_id,
        chrono::Utc::now().timestamp(),
        signer,
        ctx.attachment_store.as_ref(),
        &ctx.attachment_cache,
        ctx.cfg.attachment_max_bytes as u64,
    )
    .await?;
    if report != ExpandReport::default() {
        tracing::info!(
            target: "nasiko::llm_router::attachments",
            %agent_id,
            attached = report.attached,
            skipped_media_present = report.skipped_media_present,
            skipped_text_present = report.skipped_text_present,
            repeated = report.repeated,
            invalid = report.invalid,
            bytes = report.bytes,
            cache_hits = report.cache_hits,
            "attachments: marked files placed into the model call"
        );
    }
    Ok(())
}

enum Action {
    Attach(usize),
    Repeat(usize),
    Remove,
}

struct Cut {
    line: Range<usize>,
    action: Action,
}

struct TextEdit {
    part: Option<usize>,
    cuts: Vec<Cut>,
}

fn current_turn(messages: &[Message]) -> Vec<usize> {
    let opens_turn =
        |i: usize| messages[i].role == "user" && (i == 0 || messages[i - 1].role != "tool");
    let Some(start) = (0..messages.len())
        .rev()
        .find(|&i| opens_turn(i))
        .or_else(|| messages.iter().rposition(|m| m.role == "user"))
    else {
        return Vec::new();
    };
    (start..messages.len())
        .filter(|&i| matches!(messages[i].role.as_str(), "user" | "tool"))
        .collect()
}

fn mentions_token(message: &Message) -> bool {
    match &message.content {
        Some(Value::String(text)) => text.contains(TOKEN_PREFIX),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .any(|text| text.contains(TOKEN_PREFIX)),
        _ => false,
    }
}

fn has_media(message: &Message) -> bool {
    matches!(&message.content, Some(Value::Array(parts))
        if parts.iter().any(|p| p.get("type").and_then(Value::as_str) != Some("text")))
}

fn already_pasted(messages: &[Message], file: &MarkerClaims, base64: &str) -> bool {
    if !is_text_like(&normalized_media_type(&file.media_type)) {
        return false;
    }
    let Some(probe) = text_probe(base64) else {
        return false;
    };
    let crlf = probe.contains('\n').then(|| probe.replace('\n', "\r\n"));
    messages.iter().flat_map(message_texts).any(|text| {
        text.contains(probe.as_str()) || crlf.as_deref().is_some_and(|p| text.contains(p))
    })
}

fn text_probe(base64: &str) -> Option<String> {
    let head = &base64[..base64.len().min(PROBE_READ_BYTES.div_ceil(3) * 4)];
    let bytes = STANDARD.decode(head).ok()?;
    let bytes = bytes.strip_prefix(UTF8_BOM).unwrap_or(&bytes);
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) if e.error_len().is_none() => std::str::from_utf8(&bytes[..e.valid_up_to()]).ok()?,
        Err(_) => return None,
    };
    let text = text.replace("\r\n", "\n");
    let text = text.trim();
    let mut end = text.len().min(PROBE_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let probe = text[..end].trim_end();
    (probe.len() >= MIN_PROBE_BYTES).then(|| probe.to_string())
}

fn message_texts(message: &Message) -> impl Iterator<Item = &str> {
    let (whole, parts) = match &message.content {
        Some(Value::String(text)) => (Some(text.as_str()), None),
        Some(Value::Array(parts)) => (
            None,
            Some(
                parts
                    .iter()
                    .filter_map(|p| p.get("text").and_then(Value::as_str)),
            ),
        ),
        _ => (None, None),
    };
    whole.into_iter().chain(parts.into_iter().flatten())
}

fn normalized_media_type(declared: &str) -> String {
    declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn scannable_texts(message: &Message) -> Vec<(Option<usize>, &str, usize)> {
    let texts: Vec<(Option<usize>, &str)> = match &message.content {
        Some(Value::String(text)) => vec![(None, text.as_str())],
        Some(Value::Array(parts)) => parts
            .iter()
            .enumerate()
            .filter(|(_, p)| p.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|(i, p)| p.get("text").and_then(Value::as_str).map(|t| (Some(i), t)))
            .collect(),
        _ => Vec::new(),
    };
    if message.role != "user" {
        return texts
            .into_iter()
            .map(|(part, text)| (part, text, 0))
            .collect();
    }
    let cutoff = texts.iter().enumerate().rev().find_map(|(n, (_, text))| {
        text.rfind(CURRENT_MESSAGE)
            .map(|pos| (n, pos + CURRENT_MESSAGE.len()))
    });
    texts
        .into_iter()
        .enumerate()
        .filter_map(|(n, (part, text))| match cutoff {
            Some((at, _)) if n < at => None,
            Some((at, from)) if n == at => Some((part, text, from)),
            _ => Some((part, text, 0)),
        })
        .collect()
}

async fn fetch_one(
    file: &MarkerClaims,
    store: &dyn AttachmentStore,
    cache: &AttachmentCache,
) -> Result<(Arc<str>, bool), GatewayError> {
    cache
        .get_or_fetch(&file.reference, store)
        .await
        .map(|got| (got.base64, got.from_cache))
        .map_err(|e| fetch_failure(file, e))
}

fn fetch_failure(file: &MarkerClaims, error: AttachmentFetchError) -> GatewayError {
    match error {
        AttachmentFetchError::NotFound => GatewayError::BadRequest(format!(
            "attached file '{}' is no longer available",
            file.filename
        )),
        AttachmentFetchError::Unsupported => GatewayError::BadRequest(format!(
            "attached file '{}' cannot be read on this server",
            file.filename
        )),
        AttachmentFetchError::Storage(detail) => {
            tracing::warn!(
                target: "nasiko::llm_router::attachments",
                reference = ?file.reference,
                error = %detail,
                "attachments: reading an attached file failed"
            );
            GatewayError::StorageUnavailable
        }
    }
}

fn media_part(file: &MarkerClaims, base64: &str) -> Value {
    let declared = normalized_media_type(&file.media_type);
    let media_type = if declared.is_empty() {
        "application/octet-stream"
    } else {
        declared.as_str()
    };
    let data_url = || format!("data:{media_type};base64,{base64}");
    match media_type.split_once('/') {
        Some(("image", _)) => json!({"type": "image_url", "image_url": {"url": data_url()}}),
        Some(("audio", subtype)) => json!({
            "type": "input_audio",
            "input_audio": {"data": base64, "format": audio_format(subtype)},
        }),
        Some(("video", _)) => json!({"type": "video_url", "video_url": {"url": data_url()}}),
        _ => {
            let name = file.filename.trim();
            json!({
                "type": "file",
                "file": {
                    "file_data": data_url(),
                    "filename": if name.is_empty() { "document" } else { name },
                },
            })
        }
    }
}

fn audio_format(subtype: &str) -> &str {
    match subtype {
        "mpeg" | "mp3" => "mp3",
        "wav" | "x-wav" | "wave" | "vnd.wave" => "wav",
        other => other.trim_start_matches("x-"),
    }
}

fn apply(
    message: &mut Message,
    texts: Vec<TextEdit>,
    files: &[MarkerClaims],
    media: &mut [Option<Value>],
) {
    let attaches = texts
        .iter()
        .flat_map(|t| &t.cuts)
        .any(|cut| matches!(cut.action, Action::Attach(_)));
    match message.content.take() {
        Some(Value::String(text)) => {
            let edit = texts.into_iter().next().map(|t| t.cuts).unwrap_or_default();
            message.content = Some(if attaches {
                Value::Array(split(&text, &edit, files, media, Map::new()))
            } else {
                Value::String(rewritten(&text, &edit, files))
            });
        }
        Some(Value::Array(parts)) => {
            let mut by_part: HashMap<usize, Vec<Cut>> = texts
                .into_iter()
                .filter_map(|t| t.part.map(|p| (p, t.cuts)))
                .collect();
            let mut out = Vec::with_capacity(parts.len() + files.len());
            for (i, part) in parts.into_iter().enumerate() {
                match by_part.remove(&i) {
                    Some(cuts) => {
                        let Value::Object(mut fields) = part else {
                            continue;
                        };
                        let text = match fields.remove("text") {
                            Some(Value::String(text)) => text,
                            _ => String::new(),
                        };
                        fields.remove("type");
                        out.extend(split(&text, &cuts, files, media, fields));
                    }
                    None => out.push(part),
                }
            }
            message.content = Some(Value::Array(out));
        }
        other => message.content = other,
    }
}

fn rewritten(text: &str, cuts: &[Cut], files: &[MarkerClaims]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut from = 0;
    for cut in cuts {
        out.push_str(&text[from..cut.line.start]);
        if let Action::Repeat(slot) = cut.action {
            out.push_str(&shown_above_line(&files[slot]));
        }
        from = cut.line.end;
    }
    out.push_str(&text[from..]);
    out
}

fn split(
    text: &str,
    cuts: &[Cut],
    files: &[MarkerClaims],
    media: &mut [Option<Value>],
    extras: Map<String, Value>,
) -> Vec<Value> {
    let mut pieces = Vec::new();
    let mut buffer = String::new();
    let mut from = 0;
    for cut in cuts {
        buffer.push_str(&text[from..cut.line.start]);
        match cut.action {
            Action::Attach(slot) => {
                flush_text(&mut buffer, &mut pieces);
                pieces.extend(media[slot].take());
            }
            Action::Repeat(slot) => buffer.push_str(&shown_above_line(&files[slot])),
            Action::Remove => {}
        }
        from = cut.line.end;
    }
    buffer.push_str(&text[from..]);
    flush_text(&mut buffer, &mut pieces);
    if let Some(Value::Object(last)) = pieces.last_mut() {
        last.extend(extras);
    }
    pieces
}

fn flush_text(buffer: &mut String, pieces: &mut Vec<Value>) {
    if !buffer.trim().is_empty() {
        pieces.push(json!({"type": "text", "text": std::mem::take(buffer)}));
    }
    buffer.clear();
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    use bytes::Bytes;
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::*;
    use crate::attachments::cache::ATTACHMENT_CACHE_TTL;
    use crate::attachments::marker::{AttachmentRef, MARKER_TTL_SECS, MarkerClaims};
    use crate::attachments::store::AttachmentFetchError;
    use crate::ir::{ContentPart, DataSource};

    const FLOW: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
    const NOW: i64 = 1_800_000_000;
    const LIMIT: u64 = 30 * 1024 * 1024;
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-png";
    const PDF: &[u8] = b"%PDF-1.7 fake";

    fn signer() -> MarkerSigner {
        MarkerSigner::from_secret("expand-test-secret").unwrap()
    }

    #[derive(Default)]
    struct FakeStore {
        files: HashMap<AttachmentRef, Result<Bytes, &'static str>>,
        fetched: Mutex<Vec<AttachmentRef>>,
    }

    impl FakeStore {
        fn with(mut self, reference: AttachmentRef, bytes: &'static [u8]) -> Self {
            self.files.insert(reference, Ok(Bytes::from_static(bytes)));
            self
        }

        fn failing(mut self, reference: AttachmentRef, how: &'static str) -> Self {
            self.files.insert(reference, Err(how));
            self
        }

        fn fetch_count(&self) -> usize {
            self.fetched.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl AttachmentStore for FakeStore {
        async fn fetch(&self, reference: &AttachmentRef) -> Result<Bytes, AttachmentFetchError> {
            self.fetched.lock().unwrap().push(reference.clone());
            match self.files.get(reference) {
                Some(Ok(bytes)) => Ok(bytes.clone()),
                Some(Err("storage")) => Err(AttachmentFetchError::Storage("down".into())),
                Some(Err("unsupported")) => Err(AttachmentFetchError::Unsupported),
                _ => Err(AttachmentFetchError::NotFound),
            }
        }
    }

    fn chat_file(n: u128) -> AttachmentRef {
        AttachmentRef::ChatUpload {
            session_id: "sess".into(),
            file_id: Uuid::from_u128(n),
        }
    }

    fn workspace_file(n: u128) -> AttachmentRef {
        AttachmentRef::Workspace {
            agent_id: Uuid::from_u128(1),
            user_id: Uuid::from_u128(2),
            blob_id: Uuid::from_u128(n),
        }
    }

    fn claims(reference: AttachmentRef, name: &str, media_type: &str, size: u64) -> MarkerClaims {
        MarkerClaims {
            reference,
            filename: name.into(),
            media_type: media_type.into(),
            size_bytes: size,
            flow_id: FLOW.into(),
            expires_at: NOW + MARKER_TTL_SECS,
        }
    }

    fn png_line(n: u128) -> String {
        signer().marker_line(&claims(
            chat_file(n),
            "fact.png",
            "image/png",
            PNG.len() as u64,
        ))
    }

    fn pdf_line(n: u128) -> String {
        signer().marker_line(&claims(
            chat_file(n),
            "q3.pdf",
            "application/pdf",
            PDF.len() as u64,
        ))
    }

    fn request(messages: Value) -> ChatRequest {
        serde_json::from_value(json!({ "model": "m", "messages": messages })).unwrap()
    }

    fn cache() -> AttachmentCache {
        AttachmentCache::new(1024 * 1024, ATTACHMENT_CACHE_TTL)
    }

    async fn run(req: &mut ChatRequest, store: &FakeStore) -> Result<ExpandReport, GatewayError> {
        expand(req, FLOW, NOW, &signer(), store, &cache(), LIMIT).await
    }

    fn content(req: &ChatRequest, index: usize) -> Value {
        req.messages[index].content.clone().unwrap()
    }

    fn data_url(media_type: &str, bytes: &[u8]) -> String {
        format!("data:{media_type};base64,{}", STANDARD.encode(bytes))
    }

    #[tokio::test]
    async fn a_request_without_markers_is_left_exactly_as_it_was() {
        let messages = json!([
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": [{"type": "text", "text": "hi"}, {"type": "image_url", "image_url": {"url": "https://x.test/a.png"}}]},
        ]);
        let mut req = request(messages.clone());
        let store = FakeStore::default();
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report, ExpandReport::default());
        assert_eq!(serde_json::to_value(&req.messages).unwrap(), messages);
        assert_eq!(store.fetch_count(), 0);
    }

    #[tokio::test]
    async fn a_marked_image_in_the_user_message_becomes_an_image_part() {
        let mut req = request(json!([
            {"role": "user", "content": format!("What is in this?\n{}", png_line(1))},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(
            content(&req, 0),
            json!([
                {"type": "text", "text": "What is in this?\n"},
                {"type": "image_url", "image_url": {"url": data_url("image/png", PNG)}},
            ])
        );
        assert_eq!(report.attached, 1);
        assert_eq!(report.bytes, PNG.len() as u64);
        let parts: Vec<ContentPart> = content(&req, 0)
            .as_array()
            .unwrap()
            .iter()
            .map(|p| ContentPart::from_openai_part(p).unwrap())
            .collect();
        assert!(
            matches!(&parts[1], ContentPart::Image { source: DataSource::Inline { media_type, .. }, .. } if media_type == "image/png")
        );
    }

    #[tokio::test]
    async fn each_media_type_gets_the_part_shape_the_providers_read() {
        let cases: [(&str, &str, Value); 6] = [
            (
                "q3.pdf",
                "application/pdf",
                json!({"type": "file", "file": {"file_data": data_url("application/pdf", PDF), "filename": "q3.pdf"}}),
            ),
            (
                "notes.csv",
                "text/csv; charset=utf-8",
                json!({"type": "file", "file": {"file_data": data_url("text/csv", PDF), "filename": "notes.csv"}}),
            ),
            (
                "tone.mp3",
                "audio/mpeg",
                json!({"type": "input_audio", "input_audio": {"data": STANDARD.encode(PDF), "format": "mp3"}}),
            ),
            (
                "tone.wav",
                "audio/x-wav",
                json!({"type": "input_audio", "input_audio": {"data": STANDARD.encode(PDF), "format": "wav"}}),
            ),
            (
                "clip.mp4",
                "video/mp4",
                json!({"type": "video_url", "video_url": {"url": data_url("video/mp4", PDF)}}),
            ),
            (
                "",
                "",
                json!({"type": "file", "file": {"file_data": data_url("application/octet-stream", PDF), "filename": "document"}}),
            ),
        ];
        for (name, media_type, expected) in cases {
            let line =
                signer().marker_line(&claims(chat_file(9), name, media_type, PDF.len() as u64));
            let mut req = request(json!([{"role": "user", "content": line}]));
            let store = FakeStore::default().with(chat_file(9), PDF);
            run(&mut req, &store).await.unwrap();
            assert_eq!(content(&req, 0), json!([expected]), "{media_type}");
            ContentPart::from_openai_part(&content(&req, 0)[0]).unwrap();
        }
    }

    #[tokio::test]
    async fn a_marker_in_a_tool_result_of_this_turn_is_placed_in_that_tool_message() {
        let ws = signer().marker_line(&claims(
            workspace_file(5),
            "fact.png",
            "image/png",
            PNG.len() as u64,
        ));
        let mut req = request(json!([
            {"role": "user", "content": "describe img/fact.png"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read_file", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "c1", "content": format!("Image `img/fact.png`:\n{ws}")},
        ]));
        let store = FakeStore::default().with(workspace_file(5), PNG);
        run(&mut req, &store).await.unwrap();
        assert_eq!(
            content(&req, 2),
            json!([
                {"type": "text", "text": "Image `img/fact.png`:\n"},
                {"type": "image_url", "image_url": {"url": data_url("image/png", PNG)}},
            ])
        );
        assert_eq!(content(&req, 0), json!("describe img/fact.png"));
    }

    #[tokio::test]
    async fn markers_from_earlier_turns_stay_text_and_are_never_fetched() {
        let old_user = format!("first question\n{}", png_line(1));
        let old_tool = pdf_line(2);
        let mut req = request(json!([
            {"role": "user", "content": old_user},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "c1", "content": old_tool},
            {"role": "assistant", "content": "done"},
            {"role": "user", "content": "and now?"},
        ]));
        let store = FakeStore::default()
            .with(chat_file(1), PNG)
            .with(chat_file(2), PDF);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report, ExpandReport::default());
        assert_eq!(content(&req, 0), json!(old_user));
        assert_eq!(content(&req, 2), json!(old_tool));
        assert_eq!(store.fetch_count(), 0);
    }

    #[tokio::test]
    async fn only_text_after_the_current_message_label_is_this_turn() {
        let history = format!("History:\nuser: earlier {}", png_line(1));
        let mut req = request(json!([
            {"role": "user", "content": format!("{history}\n\nCurrent message: and this one\n{}", pdf_line(2))},
        ]));
        let store = FakeStore::default()
            .with(chat_file(1), PNG)
            .with(chat_file(2), PDF);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.attached, 1);
        assert_eq!(*store.fetched.lock().unwrap(), vec![chat_file(2)]);
        let parts = content(&req, 0);
        assert_eq!(
            parts[0]["text"],
            json!(format!("{history}\n\nCurrent message: and this one\n"))
        );
        assert_eq!(parts[1]["type"], "file");
    }

    #[tokio::test]
    async fn text_sent_beside_tool_results_does_not_end_the_turn() {
        let ws = signer().marker_line(&claims(
            workspace_file(5),
            "fact.png",
            "image/png",
            PNG.len() as u64,
        ));
        let mut req = request(json!([
            {"role": "user", "content": format!("read both\n{}", pdf_line(2))},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read_file", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "c1", "content": ws},
            {"role": "user", "content": "keep going"},
        ]));
        let store = FakeStore::default()
            .with(chat_file(2), PDF)
            .with(workspace_file(5), PNG);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.attached, 2);
        assert_eq!(content(&req, 0)[1]["type"], "file");
        assert_eq!(content(&req, 2)[0]["type"], "image_url");
        assert_eq!(content(&req, 3), json!("keep going"));
    }

    #[tokio::test]
    async fn an_agent_that_attached_media_itself_gets_the_line_removed_and_nothing_fetched() {
        let mut req = request(json!([
            {"role": "user", "content": [
                {"type": "text", "text": format!("What is this?\n{}", png_line(1))},
                {"type": "image_url", "image_url": {"url": data_url("image/png", PNG)}},
            ]},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.skipped_media_present, 1);
        assert_eq!(report.attached, 0);
        assert_eq!(store.fetch_count(), 0);
        assert_eq!(
            content(&req, 0),
            json!([
                {"type": "text", "text": "What is this?\n"},
                {"type": "image_url", "image_url": {"url": data_url("image/png", PNG)}},
            ])
        );
    }

    const CSV: &[u8] = b"region,revenue\nEMEA,913\nAPAC,402\nAMER,1188\nLATAM,77\n";
    const CSV_CRLF: &[u8] = b"region,revenue\r\nEMEA,913\r\nAPAC,402\r\nAMER,1188\r\nLATAM,77\r\n";

    fn text_line(n: u128, media_type: &str, bytes: &[u8]) -> String {
        signer().marker_line(&claims(
            chat_file(n),
            "sales.csv",
            media_type,
            bytes.len() as u64,
        ))
    }

    fn pasted(bytes: &[u8]) -> String {
        format!(
            "--- File: sales.csv (text/csv) ---\n{}",
            String::from_utf8_lossy(bytes)
        )
    }

    #[tokio::test]
    async fn a_text_file_the_agent_already_pasted_is_not_attached_again() {
        let mut req = request(json!([
            {"role": "user", "content": [
                {"type": "text", "text": format!("Totals?\n{}", text_line(1, "text/csv", CSV))},
                {"type": "text", "text": pasted(CSV)},
            ]},
        ]));
        let store = FakeStore::default().with(chat_file(1), CSV);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.attached, 0);
        assert_eq!(report.skipped_text_present, 1);
        assert_eq!(report.bytes, 0);
        assert_eq!(
            content(&req, 0),
            json!([
                {"type": "text", "text": "Totals?\n"},
                {"type": "text", "text": pasted(CSV)},
            ])
        );
    }

    #[tokio::test]
    async fn a_pasted_text_file_is_found_whatever_its_line_endings() {
        for (stored, sent) in [(CSV_CRLF, CSV), (CSV, CSV_CRLF), (CSV_CRLF, CSV_CRLF)] {
            let mut req = request(json!([
                {"role": "user", "content": format!("Totals?\n{}\n{}", text_line(1, "text/csv", stored), pasted(sent))},
            ]));
            let store = FakeStore::default().with(chat_file(1), stored);
            let report = run(&mut req, &store).await.unwrap();
            assert_eq!(report.skipped_text_present, 1);
            assert_eq!(
                content(&req, 0),
                json!(format!("Totals?\n\n{}", pasted(sent)))
            );
        }
    }

    #[tokio::test]
    async fn a_text_file_with_a_byte_order_mark_is_found_when_pasted_without_it() {
        let with_bom = [b"\xEF\xBB\xBF".as_slice(), CSV].concat().leak();
        let mut req = request(json!([
            {"role": "user", "content": format!("{}\n{}", text_line(1, "text/csv", with_bom), pasted(CSV))},
        ]));
        let store = FakeStore::default().with(chat_file(1), with_bom);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.skipped_text_present, 1);
        assert_eq!(report.attached, 0);
    }

    #[tokio::test]
    async fn a_text_file_pasted_in_the_system_prompt_is_not_attached_again() {
        let mut req = request(json!([
            {"role": "system", "content": pasted(CSV)},
            {"role": "user", "content": format!("Totals?\n{}", text_line(1, "application/json; charset=utf-8", CSV))},
        ]));
        let store = FakeStore::default().with(chat_file(1), CSV);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.skipped_text_present, 1);
        assert_eq!(content(&req, 1), json!("Totals?\n"));
    }

    #[tokio::test]
    async fn a_second_marker_of_a_pasted_text_file_is_removed_too() {
        let line = text_line(1, "text/csv", CSV);
        let mut req = request(json!([
            {"role": "user", "content": format!("{line}\nagain: {line}\n{}", pasted(CSV))},
        ]));
        let store = FakeStore::default().with(chat_file(1), CSV);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.attached, 0);
        assert_eq!(
            content(&req, 0),
            json!(format!("\nagain: \n{}", pasted(CSV)))
        );
    }

    #[tokio::test]
    async fn a_text_file_the_agent_did_not_paste_is_still_attached() {
        let other = b"region,revenue\nEMEA,914\nAPAC,402\nAMER,1188\nLATAM,77\n";
        let mut req = request(json!([
            {"role": "user", "content": format!("Totals?\n{}\n{}", text_line(1, "text/csv", CSV), pasted(other))},
        ]));
        let store = FakeStore::default().with(chat_file(1), CSV);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.attached, 1);
        assert_eq!(report.skipped_text_present, 0);
        assert_eq!(report.bytes, CSV.len() as u64);
        let parts = content(&req, 0);
        assert_eq!(parts[1]["type"], "file");
        assert_eq!(parts[1]["file"]["file_data"], data_url("text/csv", CSV));
    }

    #[tokio::test]
    async fn only_text_files_long_enough_to_recognise_are_checked() {
        let short = b"EMEA,913\n";
        let not_utf8 = b"region,revenue\nEMEA,913\xFF\xFEAPAC,402\nAMER,1188\nLATAM,77\n";
        for (media_type, bytes, sent) in [
            ("text/csv", short.as_slice(), pasted(short)),
            ("text/plain", not_utf8.as_slice(), pasted(not_utf8)),
            ("application/octet-stream", CSV, pasted(CSV)),
        ] {
            let mut req = request(json!([
                {"role": "user", "content": format!("{}\n{sent}", text_line(1, media_type, bytes))},
            ]));
            let store = FakeStore::default().with(chat_file(1), bytes);
            let report = run(&mut req, &store).await.unwrap();
            assert_eq!(report.attached, 1, "{media_type}");
            assert_eq!(report.skipped_text_present, 0, "{media_type}");
        }
    }

    #[tokio::test]
    async fn a_pasted_text_file_beside_an_image_leaves_the_image_attached() {
        let mut req = request(json!([
            {"role": "user", "content": format!("{}\n{}\n{}", png_line(2), text_line(1, "text/csv", CSV), pasted(CSV))},
        ]));
        let store = FakeStore::default()
            .with(chat_file(1), CSV)
            .with(chat_file(2), PNG);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.attached, 1);
        assert_eq!(report.skipped_text_present, 1);
        assert_eq!(report.bytes, PNG.len() as u64);
        assert_eq!(
            content(&req, 0),
            json!([
                {"type": "image_url", "image_url": {"url": data_url("image/png", PNG)}},
                {"type": "text", "text": format!("\n\n{}", pasted(CSV))},
            ])
        );
    }

    #[tokio::test]
    async fn the_same_file_twice_is_sent_once_and_named_the_second_time() {
        let mut req = request(json!([
            {"role": "user", "content": format!("{}\nagain: {}", png_line(1), png_line(1))},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.attached, 1);
        assert_eq!(report.repeated, 1);
        assert_eq!(store.fetch_count(), 1);
        assert_eq!(
            content(&req, 0),
            json!([
                {"type": "image_url", "image_url": {"url": data_url("image/png", PNG)}},
                {"type": "text", "text": "\nagain: [Attached file: fact.png (shown above)]"},
            ])
        );
    }

    #[tokio::test]
    async fn two_files_keep_their_order_and_blank_text_between_them_is_dropped() {
        let mut req = request(json!([
            {"role": "user", "content": format!("{}\n{}", png_line(1), pdf_line(2))},
        ]));
        let store = FakeStore::default()
            .with(chat_file(1), PNG)
            .with(chat_file(2), PDF);
        run(&mut req, &store).await.unwrap();
        let parts = content(&req, 0);
        assert_eq!(parts.as_array().unwrap().len(), 2);
        assert_eq!(parts[0]["type"], "image_url");
        assert_eq!(parts[1]["type"], "file");
    }

    #[tokio::test]
    async fn files_over_the_limit_are_refused_by_size_before_any_fetch() {
        let big = signer().marker_line(&claims(
            chat_file(1),
            "big.pdf",
            "application/pdf",
            20 * 1024 * 1024,
        ));
        let bigger = signer().marker_line(&claims(
            chat_file(2),
            "bigger.pdf",
            "application/pdf",
            11 * 1024 * 1024,
        ));
        let mut req = request(json!([{"role": "user", "content": format!("{big}\n{bigger}")}]));
        let store = FakeStore::default();
        let err = run(&mut req, &store).await.unwrap_err();
        match err {
            GatewayError::BadRequest(message) => assert_eq!(
                message,
                "the attached files total 31.0 MB, over the 30.0 MB limit for one model call"
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(store.fetch_count(), 0);
    }

    #[tokio::test]
    async fn a_missing_or_unreadable_file_is_named_and_storage_trouble_is_a_503() {
        let line = pdf_line(1);
        let cases = [
            ("missing", "attached file 'q3.pdf' is no longer available"),
            (
                "unsupported",
                "attached file 'q3.pdf' cannot be read on this server",
            ),
        ];
        for (how, expected) in cases {
            let mut req = request(json!([{"role": "user", "content": line.clone()}]));
            let store = FakeStore::default().failing(chat_file(1), how);
            match run(&mut req, &store).await.unwrap_err() {
                GatewayError::BadRequest(message) => assert_eq!(message, expected),
                other => panic!("{how}: {other:?}"),
            }
        }
        let mut req = request(json!([{"role": "user", "content": line}]));
        let store = FakeStore::default().failing(chat_file(1), "storage");
        assert!(matches!(
            run(&mut req, &store).await.unwrap_err(),
            GatewayError::StorageUnavailable
        ));
    }

    #[tokio::test]
    async fn tokens_that_do_not_verify_stay_text_and_are_counted() {
        let mut other_flow = claims(chat_file(1), "a.png", "image/png", 3);
        other_flow.flow_id = "00000000000000000000000000000000".into();
        let mut expired = claims(chat_file(2), "b.png", "image/png", 3);
        expired.expires_at = NOW - 1;
        let foreign = MarkerSigner::from_secret("someone-else")
            .unwrap()
            .marker_line(&claims(chat_file(3), "c.png", "image/png", 3));
        let text = format!(
            "{}\n{}\n{foreign}",
            signer().marker_line(&other_flow),
            signer().marker_line(&expired)
        );
        let mut req = request(json!([{"role": "user", "content": text.clone()}]));
        let store = FakeStore::default();
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.invalid, 3);
        assert_eq!(report.attached, 0);
        assert_eq!(content(&req, 0), json!(text));
        assert_eq!(store.fetch_count(), 0);
    }

    #[tokio::test]
    async fn a_cache_marker_on_the_text_stays_on_the_last_piece() {
        let marker = json!({"type": "ephemeral"});
        let mut req = request(json!([
            {"role": "user", "content": [
                {"type": "text", "text": format!("look\n{}\nthanks", png_line(1)), "cache_control": marker},
            ]},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        run(&mut req, &store).await.unwrap();
        assert_eq!(
            content(&req, 0),
            json!([
                {"type": "text", "text": "look\n"},
                {"type": "image_url", "image_url": {"url": data_url("image/png", PNG)}},
                {"type": "text", "text": "\nthanks", "cache_control": marker},
            ])
        );
    }

    #[tokio::test]
    async fn a_marker_inside_a_quoted_json_tool_result_is_still_found() {
        let ws = signer().marker_line(&claims(
            workspace_file(5),
            "fact.png",
            "image/png",
            PNG.len() as u64,
        ));
        let quoted = format!(
            r#"{{"content":[{{"type":"text","text":{}}}]}}"#,
            serde_json::to_string(&format!("Image:\n{ws}")).unwrap()
        );
        let mut req = request(json!([
            {"role": "user", "content": "read it"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "read_file", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "c1", "content": quoted},
        ]));
        let store = FakeStore::default().with(workspace_file(5), PNG);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report.attached, 1);
        let parts = content(&req, 2);
        assert_eq!(
            parts[0]["text"],
            r#"{"content":[{"type":"text","text":"Image:\n"#
        );
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[2]["text"], r#""}]}"#);
    }

    #[tokio::test]
    async fn a_second_call_in_the_same_turn_reads_the_file_from_the_cache() {
        let cache = cache();
        let store = FakeStore::default().with(chat_file(1), PNG);
        for expected_hits in [0, 1] {
            let mut req = request(json!([{"role": "user", "content": png_line(1)}]));
            let report = expand(&mut req, FLOW, NOW, &signer(), &store, &cache, LIMIT)
                .await
                .unwrap();
            assert_eq!(report.cache_hits, expected_hits);
        }
        assert_eq!(store.fetch_count(), 1);
    }

    fn roles(req: &ChatRequest) -> Vec<&str> {
        req.messages.iter().map(|m| m.role.as_str()).collect()
    }

    fn image_part() -> Value {
        json!({"type": "image_url", "image_url": {"url": data_url("image/png", PNG)}})
    }

    fn pdf_part() -> Value {
        json!({"type": "file", "file": {"file_data": data_url("application/pdf", PDF), "filename": "q3.pdf"}})
    }

    #[tokio::test]
    async fn a_marker_in_the_system_prompt_is_attached_to_the_user_message_of_this_turn() {
        let mut req = request(json!([
            {"role": "system", "content": format!("Summarise the document.\n{}", pdf_line(1))},
            {"role": "user", "content": "go"},
        ]));
        let store = FakeStore::default().with(chat_file(1), PDF);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(roles(&req), ["system", "user"]);
        assert_eq!(content(&req, 0), json!("Summarise the document.\n"));
        assert_eq!(
            content(&req, 1),
            json!([{"type": "text", "text": "go"}, pdf_part()])
        );
        assert_eq!(report.attached, 1);
        assert_eq!(report.bytes, PDF.len() as u64);
    }

    #[tokio::test]
    async fn a_developer_message_is_read_like_a_system_prompt() {
        let mut req = request(json!([
            {"role": "developer", "content": format!("Describe it.\n{}", png_line(1))},
            {"role": "user", "content": [{"type": "text", "text": "what is this?"}]},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        run(&mut req, &store).await.unwrap();
        assert_eq!(content(&req, 0), json!("Describe it.\n"));
        assert_eq!(
            content(&req, 1),
            json!([{"type": "text", "text": "what is this?"}, image_part()])
        );
    }

    #[tokio::test]
    async fn a_system_prompt_left_empty_by_the_cut_is_dropped() {
        let mut req = request(json!([
            {"role": "system", "content": format!("  {}\n", png_line(1))},
            {"role": "user", "content": "what is this?"},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        run(&mut req, &store).await.unwrap();
        assert_eq!(roles(&req), ["user"]);
        assert_eq!(
            content(&req, 0),
            json!([{"type": "text", "text": "what is this?"}, image_part()])
        );
    }

    #[tokio::test]
    async fn a_marker_in_a_system_prompt_made_of_parts_is_found() {
        let mut req = request(json!([
            {"role": "system", "content": [
                {"type": "text", "text": "Be brief."},
                {"type": "text", "text": png_line(1)},
            ]},
            {"role": "user", "content": "hi"},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        run(&mut req, &store).await.unwrap();
        assert_eq!(
            content(&req, 0),
            json!([{"type": "text", "text": "Be brief."}])
        );
        assert_eq!(
            content(&req, 1),
            json!([{"type": "text", "text": "hi"}, image_part()])
        );
    }

    #[tokio::test]
    async fn the_same_file_in_the_system_prompt_and_the_user_message_is_sent_once_in_the_user_spot()
    {
        let mut req = request(json!([
            {"role": "system", "content": format!("Rules.\n{}", pdf_line(1))},
            {"role": "user", "content": format!("read this\n{}\nthanks", pdf_line(1))},
        ]));
        let store = FakeStore::default().with(chat_file(1), PDF);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(content(&req, 0), json!("Rules.\n"));
        assert_eq!(
            content(&req, 1),
            json!([
                {"type": "text", "text": "read this\n"},
                pdf_part(),
                {"type": "text", "text": "\nthanks"},
            ])
        );
        assert_eq!(report.attached, 1);
        assert_eq!(report.repeated, 1);
        assert_eq!(store.fetch_count(), 1);
    }

    #[tokio::test]
    async fn a_system_marker_goes_to_the_user_message_that_opens_the_current_turn() {
        let mut req = request(json!([
            {"role": "system", "content": format!("Use the file.\n{}", png_line(1))},
            {"role": "user", "content": "first question"},
            {"role": "assistant", "content": "first answer"},
            {"role": "user", "content": "second question"},
            {"role": "assistant", "content": null, "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
            {"role": "tool", "tool_call_id": "c1", "content": "tool output"},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        run(&mut req, &store).await.unwrap();
        assert_eq!(content(&req, 1), json!("first question"));
        assert_eq!(
            content(&req, 3),
            json!([{"type": "text", "text": "second question"}, image_part()])
        );
        assert_eq!(content(&req, 5), json!("tool output"));
    }

    #[tokio::test]
    async fn a_system_marker_is_removed_unfetched_when_the_user_message_already_carries_media() {
        let mut req = request(json!([
            {"role": "system", "content": format!("Look.\n{}", png_line(1))},
            {"role": "user", "content": [{"type": "text", "text": "this"}, image_part()]},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(content(&req, 0), json!("Look.\n"));
        assert_eq!(
            content(&req, 1),
            json!([{"type": "text", "text": "this"}, image_part()])
        );
        assert_eq!(report.skipped_media_present, 1);
        assert_eq!(store.fetch_count(), 0);
    }

    #[tokio::test]
    async fn a_system_marker_that_does_not_verify_stays_text_and_is_counted() {
        let other_flow = MarkerClaims {
            flow_id: "0".repeat(32),
            ..claims(chat_file(1), "fact.png", "image/png", PNG.len() as u64)
        };
        let line = signer().marker_line(&other_flow);
        let mut req = request(json!([
            {"role": "system", "content": format!("Look.\n{line}")},
            {"role": "user", "content": "this"},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(content(&req, 0), json!(format!("Look.\n{line}")));
        assert_eq!(content(&req, 1), json!("this"));
        assert_eq!(report.invalid, 1);
        assert_eq!(store.fetch_count(), 0);
    }

    #[tokio::test]
    async fn a_system_only_request_gets_a_user_message_holding_the_file() {
        let mut req = request(json!([
            {"role": "system", "content": format!("Describe the image.\n{}", png_line(1))},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(roles(&req), ["system", "user"]);
        assert_eq!(content(&req, 0), json!("Describe the image.\n"));
        assert_eq!(content(&req, 1), json!([image_part()]));
        assert_eq!(report.attached, 1);
    }

    #[tokio::test]
    async fn a_text_file_pasted_in_the_user_message_is_not_attached_again_from_the_system_prompt() {
        let csv = "region,q2\nEMEA,913\nAPAC,402\nAMER,777\nLATAM,120\n";
        let line = signer().marker_line(&claims(
            chat_file(3),
            "data.csv",
            "text/csv",
            csv.len() as u64,
        ));
        let mut req = request(json!([
            {"role": "system", "content": format!("Use the data.\n{line}")},
            {"role": "user", "content": format!("Totals?\n{csv}")},
        ]));
        let store = FakeStore::default().with(chat_file(3), csv.as_bytes().to_vec().leak());
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(content(&req, 0), json!("Use the data.\n"));
        assert_eq!(content(&req, 1), json!(format!("Totals?\n{csv}")));
        assert_eq!(report.skipped_text_present, 1);
        assert_eq!(report.attached, 0);
    }

    #[tokio::test]
    async fn assistant_messages_are_never_scanned() {
        let mut req = request(json!([
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": format!("echo {}", png_line(1))},
        ]));
        let store = FakeStore::default().with(chat_file(1), PNG);
        let report = run(&mut req, &store).await.unwrap();
        assert_eq!(report, ExpandReport::default());
        assert_eq!(store.fetch_count(), 0);
    }
}
