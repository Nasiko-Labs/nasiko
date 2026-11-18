//! The hub's content-part grammar.
//!
//! [`crate::ir::chat::Message::content`] stays a raw [`Value`] on purpose — that
//! permissiveness is what lets the OpenAI spoke forward shapes we do not model. This
//! module adds a *projection* over that raw value rather than replacing it: a spoke
//! that must translate calls [`Message::content_view`], matches on real variants, and
//! renders its own dialect. A spoke that passes bytes through untouched keeps reading
//! `content` directly and is unaffected.
//!
//! Before this existed the grammar was four `match` blocks over the string literals
//! `"text" | "image_url" | "file" | "input_audio"`, plus two verbatim copies of the
//! data-URL splitter. Adding a spoke meant reading all four and hoping. Now a new
//! [`ContentPart`] variant breaks every spoke at compile time until each one decides
//! what to do with it, which is the property the hub-and-spoke split is supposed to buy.
//!
//! Policy stays in the spokes. This module answers *what the caller sent*; whether a
//! given backend can carry it is that backend's own rule — Gemini rejects web URLs via
//! [`DataSource::inline`], Anthropic rejects audio by matching [`ContentPart::Audio`].

use super::chat::Message;
use serde_json::{Value, json};

/// A part the hub understands but some spoke cannot carry.
///
/// Deliberately neither `GatewayError` nor `ProviderError`: `ir` sits below both, and
/// each edge speaks its own error type. Inbound parsers map this onto
/// `GatewayError::BadRequest`; provider clients map it onto `ProviderError::Rejected`,
/// which surfaces as the same 400.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedPart {
    message: String,
}

impl UnsupportedPart {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn into_message(self) -> String {
        self.message
    }
}

impl std::fmt::Display for UnsupportedPart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for UnsupportedPart {}

/// Splits a base64 `data:` URL into its media type and payload.
///
/// `None` when `url` is not a data URL *or* is malformed — callers that need to tell
/// those apart check the `data:` prefix first, which is what [`DataSource::from_url_field`]
/// does to keep "not a data URL" and "broken data URL" as different errors.
pub fn split_data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (header, data) = rest.split_once(',')?;
    let at = header.len().checked_sub(BASE64_MARKER.len())?;
    header
        .get(at..)
        .filter(|tail| tail.eq_ignore_ascii_case(BASE64_MARKER))
        .map(|_| (&header[..at], data))
}

const BASE64_MARKER: &str = ";base64";

fn normalize_media_type(declared: &str) -> String {
    declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

fn data_url_name(url: &str) -> Option<&str> {
    let (header, _) = split_data_url(url)?;
    header.split(';').skip(1).find_map(|param| {
        let (key, value) = param.split_once('=')?;
        let value = value.trim();
        (["name", "filename"]
            .iter()
            .any(|k| key.trim().eq_ignore_ascii_case(k))
            && !value.is_empty())
        .then_some(value)
    })
}

/// Where a media part's bytes actually live.
///
/// Every spoke branches on exactly this question, so it is decided once here instead of
/// in four hand-rolled `starts_with("data:")` chains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataSource {
    /// Base64 payload carried in the request itself.
    Inline { media_type: String, data: String },
    /// An http(s) URL the provider is expected to fetch for itself.
    Url(String),
}

impl DataSource {
    pub fn from_inline(media_type: &str, data: &str) -> Self {
        Self::Inline {
            media_type: normalize_media_type(media_type),
            data: data.to_string(),
        }
    }

    /// Parses an OpenAI-shaped URL field (`image_url.url`, `file.file_data`).
    ///
    /// `field` names the field in the error text so the caller can tell which part of a
    /// multi-part message was malformed.
    pub fn from_url_field(url: &str, field: &str) -> Result<Self, UnsupportedPart> {
        if url.starts_with("data:") {
            let (media_type, data) = split_data_url(url).ok_or_else(|| {
                UnsupportedPart::new(format!(
                    "invalid base64 data URL in {field}: expected data:<media type>;base64,<data>"
                ))
            })?;
            let source = Self::from_inline(media_type, data);
            if source.media_type().is_some_and(str::is_empty) {
                return Err(UnsupportedPart::new(format!(
                    "invalid base64 data URL in {field}: no media type"
                )));
            }
            Ok(source)
        } else if url.starts_with("http://") || url.starts_with("https://") {
            Ok(Self::Url(url.to_string()))
        } else {
            Err(UnsupportedPart::new(format!(
                "unsupported {field} format (must be http(s) or base64 data URL)"
            )))
        }
    }

    /// Renders back to the canonical OpenAI URL field.
    pub fn to_url_field(&self) -> String {
        match self {
            Self::Inline { media_type, data } => format!("data:{media_type};base64,{data}"),
            Self::Url(url) => url.clone(),
        }
    }

    /// The inline bytes, or `None` for a URL.
    ///
    /// How a bytes-only spoke (Gemini, Bedrock) says so without re-implementing the
    /// check — and the single place to look when asking why a backend refused a URL.
    pub fn inline(&self) -> Option<(&str, &str)> {
        match self {
            Self::Inline { media_type, data } => Some((media_type, data)),
            Self::Url(_) => None,
        }
    }

    /// The declared media type, when the bytes are inline.
    pub fn media_type(&self) -> Option<&str> {
        match self {
            Self::Inline { media_type, .. } => Some(media_type),
            Self::Url(_) => None,
        }
    }
}

/// One canonical content part, in the hub's (OpenAI-shaped) vocabulary.
///
/// `detail` and `filename` are modelled rather than swept into an extras bag because
/// both are load-bearing: `detail` is a real OpenAI knob the passthrough spoke must
/// round-trip, and `filename` is what Bedrock's required `document.name` is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentPart {
    Text(String),
    Image {
        source: DataSource,
        detail: Option<String>,
    },
    File {
        source: DataSource,
        filename: Option<String>,
    },
    Audio {
        data: String,
        format: String,
    },
    /// OpenAI has no video part type. `video_url` is the de-facto convention among the
    /// OpenAI-compatible endpoints that do accept video, and it mirrors `image_url`, so
    /// that is the shape the hub uses. Gemini and Bedrock carry it; Anthropic refuses it.
    Video {
        source: DataSource,
    },
}

impl ContentPart {
    /// Projects one raw OpenAI content part into the vocabulary.
    pub fn from_openai_part(p: &Value) -> Result<Self, UnsupportedPart> {
        match p.get("type").and_then(Value::as_str).unwrap_or_default() {
            "text" => Ok(Self::Text(
                p.get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )),
            "image_url" => {
                let image_url = p.get("image_url");
                let url = image_url
                    .and_then(|v| v.get("url"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| UnsupportedPart::new("image_url part missing url"))?;
                Ok(Self::Image {
                    source: DataSource::from_url_field(url, "image_url")?,
                    detail: image_url
                        .and_then(|v| v.get("detail"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
            }
            "file" => {
                let file = p.get("file");
                if let Some(file_data) = file
                    .and_then(|v| v.get("file_data"))
                    .and_then(Value::as_str)
                {
                    Ok(Self::File {
                        source: DataSource::from_url_field(file_data, "file_data")?,
                        filename: file
                            .and_then(|v| v.get("filename"))
                            .and_then(Value::as_str)
                            .or_else(|| data_url_name(file_data))
                            .map(str::to_string),
                    })
                } else if file.and_then(|v| v.get("file_id")).is_some() {
                    // The Files API is per-provider state; an id minted against one
                    // backend means nothing to another, so it cannot cross the hub.
                    Err(UnsupportedPart::new(
                        "unsupported content part type: 'file.file_id'",
                    ))
                } else {
                    Err(UnsupportedPart::new("file part missing file_data"))
                }
            }
            "input_audio" => {
                let audio = p
                    .get("input_audio")
                    .ok_or_else(|| UnsupportedPart::new("input_audio part missing data"))?;
                let data = audio
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or_else(|| UnsupportedPart::new("input_audio missing base64 data"))?;
                Ok(Self::Audio {
                    data: data.to_string(),
                    // OpenAI defaults an absent format to wav.
                    format: audio
                        .get("format")
                        .and_then(Value::as_str)
                        .unwrap_or("wav")
                        .to_string(),
                })
            }
            "video_url" => {
                let url = p
                    .get("video_url")
                    .and_then(|v| v.get("url"))
                    .and_then(Value::as_str)
                    .ok_or_else(|| UnsupportedPart::new("video_url part missing url"))?;
                Ok(Self::Video {
                    source: DataSource::from_url_field(url, "video_url")?,
                })
            }
            other => Err(UnsupportedPart::new(format!(
                "unsupported content part type: '{other}'"
            ))),
        }
    }

    /// Renders back to a raw OpenAI content part.
    pub fn to_openai_part(&self) -> Value {
        match self {
            Self::Text(text) => json!({ "type": "text", "text": text }),
            Self::Image { source, detail } => {
                let mut image_url = json!({ "url": source.to_url_field() });
                if let Some(detail) = detail {
                    image_url["detail"] = json!(detail);
                }
                json!({ "type": "image_url", "image_url": image_url })
            }
            Self::File { source, filename } => json!({
                "type": "file",
                "file": {
                    "file_data": source.to_url_field(),
                    "filename": openai_filename(source, filename.as_deref()),
                },
            }),
            Self::Audio { data, format } => json!({
                "type": "input_audio",
                "input_audio": { "data": data, "format": format },
            }),
            Self::Video { source } => json!({
                "type": "video_url",
                "video_url": { "url": source.to_url_field() },
            }),
        }
    }

    /// Non-empty text, for spokes that drop empty text parts rather than forward them.
    pub fn non_empty_text(&self) -> Option<&str> {
        match self {
            Self::Text(text) if !text.is_empty() => Some(text),
            _ => None,
        }
    }
}

/// The `file.filename` an OpenAI-shaped file part must carry.
///
/// OpenAI accepts a file part as either `file_id` **or** `file_data` + `filename`, and a
/// `file` object that satisfies neither is rejected against the *first* variant:
/// `Missing required parameter: 'messages[N].content[M].file.file_id'`. The name is
/// therefore not optional on the wire, even though it is optional in the hub — Gemini
/// `inlineData` never carries one, and an Anthropic `document` only does when the caller
/// set `title`. Rather than let those render an un-sendable part, one is synthesized from
/// the declared media type here, the single place that knows OpenAI's rule.
fn openai_filename(source: &DataSource, filename: Option<&str>) -> String {
    if let Some(name) = filename.map(str::trim).filter(|n| !n.is_empty()) {
        return name.to_string();
    }
    match source.media_type().and_then(file_extension) {
        Some(ext) => format!("document.{ext}"),
        // An extensionless name still beats omitting the field: the part reaches the
        // provider, which can then refuse it on the media type's own merits.
        None => "document".to_string(),
    }
}

/// Filename extension for a media type, over the document formats a backend will
/// actually accept. `None` rather than a guess for anything else — a wrong extension
/// reads as a real file type and is harder to diagnose than a missing one.
fn file_extension(media_type: &str) -> Option<&'static str> {
    match media_type.split(';').next()?.trim() {
        "application/pdf" => Some("pdf"),
        "application/json" => Some("json"),
        "application/msword" => Some("doc"),
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => Some("docx"),
        "application/vnd.ms-excel" => Some("xls"),
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Some("xlsx"),
        "text/plain" => Some("txt"),
        "text/markdown" => Some("md"),
        "text/csv" => Some("csv"),
        "text/html" => Some("html"),
        _ => None,
    }
}

/// A message's content, projected.
///
/// The bare-string and single-element-array cases are deliberately *not* merged: a
/// caller that sent `"hi"` gets `"hi"` forwarded, and one that sent
/// `[{"type":"text","text":"hi"}]` gets an array. Collapsing them would rewrite the
/// caller's wire shape, and on Anthropic it would also change the cacheable prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContentView {
    /// `null`, absent, or a shape we do not model.
    Empty,
    /// A bare string — the overwhelmingly common case.
    Text(String),
    /// A multimodal part array.
    Parts(Vec<ContentPart>),
}

impl Message {
    /// Projects [`Message::content`] into the part vocabulary, leaving the raw value in
    /// place. Errors name the first part that does not fit the grammar.
    pub fn content_view(&self) -> Result<ContentView, UnsupportedPart> {
        match &self.content {
            Some(Value::String(s)) => Ok(ContentView::Text(s.clone())),
            Some(Value::Array(parts)) => parts
                .iter()
                .map(ContentPart::from_openai_part)
                .collect::<Result<Vec<_>, _>>()
                .map(ContentView::Parts),
            _ => Ok(ContentView::Empty),
        }
    }
}

/// Renders parts back into a `Message::content` value, collapsing a lone text part to a
/// bare string.
///
/// That collapse is what inbound parsers want: a caller who sent one text block should
/// see `content: "hi"`, not a one-element array. Outbound converters deliberately do
/// *not* use this — they preserve whatever shape the caller sent.
pub fn parts_to_content_value(parts: &[ContentPart]) -> Value {
    if let [ContentPart::Text(text)] = parts {
        return Value::String(text.clone());
    }
    Value::Array(parts.iter().map(ContentPart::to_openai_part).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_url_round_trips_through_the_source() {
        let source = DataSource::from_url_field("data:image/png;base64,AAAA", "image_url").unwrap();
        assert_eq!(
            source,
            DataSource::Inline {
                media_type: "image/png".into(),
                data: "AAAA".into()
            }
        );
        assert_eq!(source.inline(), Some(("image/png", "AAAA")));
        assert_eq!(source.to_url_field(), "data:image/png;base64,AAAA");
    }

    #[test]
    fn web_url_stays_a_url_and_reports_no_inline_bytes() {
        let source =
            DataSource::from_url_field("https://example.com/cat.png", "image_url").unwrap();
        assert_eq!(
            source,
            DataSource::Url("https://example.com/cat.png".into())
        );
        // How a bytes-only spoke (Gemini, Bedrock) refuses a URL it cannot fetch.
        assert_eq!(source.inline(), None);
        assert_eq!(source.media_type(), None);
    }

    #[test]
    fn a_data_url_media_type_is_lowercased_and_stripped_of_parameters() {
        for url in [
            "data:Image/PNG;base64,AAAA",
            "data: image/png ;base64,AAAA",
            "data:image/png;charset=binary;base64,AAAA",
            "data:image/png;BASE64,AAAA",
        ] {
            let source = DataSource::from_url_field(url, "image_url").unwrap();

            assert_eq!(source.inline(), Some(("image/png", "AAAA")), "{url}");
            assert_eq!(source.to_url_field(), "data:image/png;base64,AAAA", "{url}");
        }
    }

    #[test]
    fn a_data_url_name_parameter_becomes_the_filename_unless_one_is_given() {
        let file = |part: Value| ContentPart::from_openai_part(&part).unwrap();
        let named = file(json!({ "type": "file", "file": {
            "file_data": "data:application/pdf;name=q3.pdf;base64,AAAA"
        }}));
        let explicit = file(json!({ "type": "file", "file": {
            "file_data": "data:application/pdf;name=q3.pdf;base64,AAAA",
            "filename": "report.pdf"
        }}));

        assert_eq!(
            named,
            ContentPart::File {
                source: DataSource::Inline {
                    media_type: "application/pdf".into(),
                    data: "AAAA".into()
                },
                filename: Some("q3.pdf".into()),
            }
        );
        assert!(
            matches!(&explicit, ContentPart::File { filename: Some(f), .. } if f == "report.pdf"),
            "{explicit:?}"
        );
    }

    #[test]
    fn a_data_url_that_is_not_base64_or_has_no_media_type_is_refused_by_name() {
        let not_base64 =
            DataSource::from_url_field("data:text/plain,hello", "file_data").unwrap_err();
        let no_type = DataSource::from_url_field("data:;base64,AAAA", "image_url").unwrap_err();

        assert!(
            not_base64
                .message()
                .contains("expected data:<media type>;base64,<data>"),
            "{not_base64}"
        );
        assert!(no_type.message().contains("no media type"), "{no_type}");
    }

    #[test]
    fn a_data_url_header_with_multibyte_characters_is_refused_without_panicking() {
        for url in ["data:éx23456,AAAA", "data:é,AAAA", "data:,AAAA"] {
            assert!(
                DataSource::from_url_field(url, "image_url").is_err(),
                "{url}"
            );
        }
    }

    #[test]
    fn malformed_and_unknown_schemes_are_distinguishable() {
        // Broken data URL — the `data:` prefix is there but the payload is not.
        let broken = DataSource::from_url_field("data:image/png,AAAA", "image_url").unwrap_err();
        assert!(broken.message().contains("invalid base64 data URL"));
        // Not a data URL at all, and not fetchable either.
        let scheme =
            DataSource::from_url_field("ftp://example.com/x.png", "image_url").unwrap_err();
        assert!(
            scheme
                .message()
                .contains("must be http(s) or base64 data URL")
        );
    }

    #[test]
    fn every_part_kind_survives_a_hub_round_trip() {
        for raw in [
            json!({ "type": "text", "text": "hi" }),
            json!({ "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } }),
            json!({ "type": "image_url", "image_url": { "url": "https://e.com/c.png", "detail": "low" } }),
            json!({ "type": "file", "file": { "file_data": "data:application/pdf;base64,JVBE", "filename": "q3.pdf" } }),
            json!({ "type": "input_audio", "input_audio": { "data": "UklG", "format": "wav" } }),
            json!({ "type": "video_url", "video_url": { "url": "data:video/mp4;base64,AAAA" } }),
            json!({ "type": "video_url", "video_url": { "url": "https://e.com/v.mp4" } }),
        ] {
            let part = ContentPart::from_openai_part(&raw).unwrap();
            assert_eq!(part.to_openai_part(), raw, "round trip lost data for {raw}");
        }
    }

    #[test]
    fn a_nameless_file_still_renders_a_filename_openai_will_accept() {
        // OpenAI rejects `{file_data}` with no `filename` by complaining about the
        // *other* variant — "Missing required parameter: '…file.file_id'" — so a
        // translated document (Gemini `inlineData`, an Anthropic `document` without a
        // `title`) must not reach it nameless.
        let part = ContentPart::File {
            source: DataSource::Inline {
                media_type: "application/pdf".into(),
                data: "JVBE".into(),
            },
            filename: None,
        };
        assert_eq!(
            part.to_openai_part(),
            json!({ "type": "file", "file": {
                "file_data": "data:application/pdf;base64,JVBE",
                "filename": "document.pdf",
            }})
        );
    }

    #[test]
    fn a_synthesized_filename_takes_the_media_types_extension() {
        let named = |media_type: &str, filename: Option<&str>| {
            openai_filename(
                &DataSource::Inline {
                    media_type: media_type.into(),
                    data: "AAAA".into(),
                },
                filename,
            )
        };
        assert_eq!(named("application/pdf", None), "document.pdf");
        assert_eq!(named("text/plain; charset=utf-8", None), "document.txt");
        // Unknown media type: no guessed extension — the provider refuses it on the
        // media type's own merits rather than on a wrong-looking name.
        assert_eq!(named("application/x-tar", None), "document");
        // A URL-sourced file has no declared media type to derive from.
        assert_eq!(
            openai_filename(&DataSource::Url("https://e.com/q3.pdf".into()), None),
            "document"
        );
        // A real name always wins; a blank one does not count as one.
        assert_eq!(named("application/pdf", Some("q3.pdf")), "q3.pdf");
        assert_eq!(named("application/pdf", Some("  ")), "document.pdf");
    }

    #[test]
    fn detail_and_filename_are_modelled_not_dropped() {
        let part = ContentPart::from_openai_part(
            &json!({ "type": "image_url", "image_url": { "url": "https://e.com/c.png", "detail": "high" } }),
        )
        .unwrap();
        match &part {
            ContentPart::Image { detail, .. } => assert_eq!(detail.as_deref(), Some("high")),
            other => panic!("expected an image, got {other:?}"),
        }
        let file = ContentPart::from_openai_part(
            &json!({ "type": "file", "file": { "file_data": "data:application/pdf;base64,JVBE", "filename": "q3.pdf" } }),
        )
        .unwrap();
        match &file {
            // Bedrock's required `document.name` is built from this.
            ContentPart::File { filename, .. } => assert_eq!(filename.as_deref(), Some("q3.pdf")),
            other => panic!("expected a file, got {other:?}"),
        }
    }

    #[test]
    fn file_id_and_unknown_types_are_rejected_by_name() {
        let file_id =
            ContentPart::from_openai_part(&json!({ "type": "file", "file": { "file_id": "f-1" } }))
                .unwrap_err();
        assert!(file_id.message().contains("file.file_id"));
        let unknown =
            ContentPart::from_openai_part(&json!({ "type": "unknown_future" })).unwrap_err();
        assert!(unknown.message().contains("unknown_future"));
        let missing =
            ContentPart::from_openai_part(&json!({ "type": "file", "file": {} })).unwrap_err();
        assert!(missing.message().contains("missing file_data"));
    }

    #[test]
    fn audio_format_defaults_to_wav() {
        let part = ContentPart::from_openai_part(
            &json!({ "type": "input_audio", "input_audio": { "data": "UklG" } }),
        )
        .unwrap();
        assert_eq!(
            part,
            ContentPart::Audio {
                data: "UklG".into(),
                format: "wav".into()
            }
        );
    }

    fn message(content: Value) -> Message {
        serde_json::from_value(json!({ "role": "user", "content": content })).unwrap()
    }

    fn message_without_content() -> Message {
        serde_json::from_value(json!({ "role": "user" })).unwrap()
    }

    #[test]
    fn content_view_keeps_a_bare_string_distinct_from_a_one_part_array() {
        assert_eq!(
            message(json!("hi")).content_view().unwrap(),
            ContentView::Text("hi".into())
        );
        assert_eq!(
            message(json!([{ "type": "text", "text": "hi" }]))
                .content_view()
                .unwrap(),
            ContentView::Parts(vec![ContentPart::Text("hi".into())])
        );
    }

    #[test]
    fn content_view_reports_null_and_unmodelled_shapes_as_empty() {
        assert_eq!(
            message_without_content().content_view().unwrap(),
            ContentView::Empty
        );
        assert_eq!(
            message(json!(null)).content_view().unwrap(),
            ContentView::Empty
        );
        assert_eq!(
            message(json!(7)).content_view().unwrap(),
            ContentView::Empty
        );
    }

    #[test]
    fn content_view_surfaces_the_first_unconvertible_part() {
        let err = message(json!([
            { "type": "text", "text": "ok" },
            { "type": "hologram", "hologram": { "url": "https://e.com/h.bin" } }
        ]))
        .content_view()
        .unwrap_err();
        assert!(err.message().contains("hologram"));
    }

    #[test]
    fn parts_collapse_to_a_bare_string_only_when_a_lone_text_part() {
        assert_eq!(
            parts_to_content_value(&[ContentPart::Text("hi".into())]),
            json!("hi")
        );
        let two = parts_to_content_value(&[
            ContentPart::Text("hi".into()),
            ContentPart::Text("there".into()),
        ]);
        assert_eq!(two.as_array().unwrap().len(), 2);
        let image = parts_to_content_value(&[ContentPart::Image {
            source: DataSource::Url("https://e.com/c.png".into()),
            detail: None,
        }]);
        assert_eq!(image.as_array().unwrap().len(), 1);
        assert_eq!(parts_to_content_value(&[]), json!([]));
    }
}
