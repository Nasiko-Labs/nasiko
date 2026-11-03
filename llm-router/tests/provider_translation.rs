//! Cross-provider translation comparison harness — **live, report-only**.
//!
//! The hub-and-spoke router lets an agent write against one provider's SDK (the
//! *client format*, X) while the LLM call is actually served by a different provider
//! (the resolved *backend*, Y). These tests don't assert that the translation is
//! "correct" — two live LLM calls for the same input legitimately differ. Instead each
//! test produces **three results for one input** so a human (the test runner) can judge
//! how the answer would have differed:
//!
//! * **Pure X** — a direct API call to the client's own provider (bypasses the
//!   router entirely). The baseline: what the agent *would have* gotten.
//! * **Pure Y** — a direct API call to the backend provider. What Y's model produces.
//! * **X → Y** — the router path via direct composition of the translation seams
//!   (`inbound_for(X).parse_chat` → `provider_for(Y).chat` →
//!   `inbound_for(X).render_chat_response`). What the agent *actually*
//!   gets in production: Y's answer, re-dressed in X's envelope.
//!
//! Same-provider pairs (X == Y) are omitted — there's nothing to compare.
//!
//! The translated path uses direct composition rather than the full HTTP router so the
//! harness needs no Postgres/JWT/agent config — only provider API keys.
//!
//! ## Running
//!
//! These are `#[ignore]`d (live network + real keys + cost). Run on demand:
//!
//! ```sh
//! export OPENAI_API_KEY=...  ANTHROPIC_API_KEY=...  GEMINI_API_KEY=...
//! cargo test -p nasiko-llm-router --test provider_translation -- --ignored --nocapture
//! ```
//!
//! A pair whose keys are absent prints a SKIP line and passes. Each pair also dumps a
//! JSON report to `target/provider_translation_report/{x}_to_{y}.json` (override the
//! directory with `NASIKO_TRANSLATION_REPORT_DIR`).
//!
//! Terminal output includes the full raw provider payload for each result by default;
//! set `NASIKO_TRANSLATION_VERBOSE=0` to print only the summarized view.
//!
//! Models are env-overridable: `OPENAI_MODEL`, `ANTHROPIC_MODEL`, `GEMINI_MODEL`.
//!
//! ## Scenarios
//!
//! Four, two of them carrying an attachment (`image_describe`, `pdf_describe`). Each
//! declares what a correct answer contains, printed above every result — because for a
//! media scenario the interesting failure is not an error, it is a *fluent answer about
//! an attachment the model never received*. Comparing the text against `Expected:` is
//! what separates those two.
//!
//! ## What this harness does not cover
//!
//! * **Bedrock.** [`Provider`] has no Bedrock arm — it uses SigV4-style auth and a
//!   different request envelope, so adding it is more than another enum variant. The
//!   Converse spoke therefore has unit coverage only, and no live confirmation that it
//!   carries image or document blocks. Stated here rather than left to be discovered.
//! * **Streaming.** Non-streaming responses only; the SSE renderers are unit-tested.
//! * **Pass/fail.** Report-only by design, and deliberately not a CI gate: it costs
//!   money per run, needs three real keys, and its output is judged by a human.

use std::fs;
use std::path::PathBuf;

use nasiko_llm_router::config::GatewayConfig;
use nasiko_llm_router::inbound::{InboundFormat, inbound_for};
use nasiko_llm_router::providers::provider_for;
use nasiko_llm_router::resolver::ResolvedConfig;
use serde::Serialize;
use serde_json::{Value, json};

/// Pinned so Pure Y and X→Y are as comparable as live models allow.
const TEMPERATURE: f64 = 0.0;
const MAX_TOKENS: i64 = 512;

// ─────────────────────────────────────────────────────────────────────────────
// Providers
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Provider {
    OpenAi,
    Anthropic,
    Gemini,
}

impl Provider {
    fn label(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
            Self::Gemini => "gemini",
        }
    }

    fn format(self) -> InboundFormat {
        match self {
            Self::OpenAi => InboundFormat::OpenAi,
            Self::Anthropic => InboundFormat::Anthropic,
            Self::Gemini => InboundFormat::Gemini,
        }
    }

    /// Default model, overridable via `{PROVIDER}_MODEL`.
    fn model(self) -> String {
        match self {
            Self::OpenAi => env_or("OPENAI_MODEL", "gpt-4o"),
            Self::Anthropic => env_or("ANTHROPIC_MODEL", "claude-haiku-4-5-20251001"),
            Self::Gemini => env_or("GEMINI_MODEL", "gemini-1.5-pro"),
        }
    }

    fn key_env(self) -> &'static str {
        match self {
            Self::OpenAi => "OPENAI_API_KEY",
            Self::Anthropic => "ANTHROPIC_API_KEY",
            Self::Gemini => "GEMINI_API_KEY",
        }
    }

    fn api_key(self) -> Option<String> {
        std::env::var(self.key_env()).ok().filter(|s| !s.is_empty())
    }

    fn base(self, cfg: &GatewayConfig) -> String {
        match self {
            Self::OpenAi => cfg.openai_api_base.clone(),
            Self::Anthropic => cfg.anthropic_api_base.clone(),
            Self::Gemini => cfg.gemini_api_base.clone(),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Scenarios — logical prompts, rendered into each provider's native request shape.
// ─────────────────────────────────────────────────────────────────────────────

struct Tool {
    name: &'static str,
    description: &'static str,
    /// JSON-schema object (kept to fields all three providers accept).
    parameters: Value,
    /// Force the model to call the tool (vs. leave it optional).
    force: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Media fixtures
// ─────────────────────────────────────────────────────────────────────────────
//
// Both carry a **verifiable fact**, which is the only reason a media scenario proves
// anything. A 1×1 transparent pixel would not: asked to describe it, a model that never
// received the attachment still answers plausibly, and the run looks like a pass. These
// have a single right answer, so a dropped attachment is visible in the text.

/// 64×64 PNG — solid blue top half, solid yellow bottom half.
const IMAGE_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAATUlEQVR42u3PMQkAAAwDsPo3vWnoWQjEQJLcOAEBAQEBAQEBAQEBAQEBAQEBAQEBAQGB1o0TEBAQEBAQEBAQEBAQEBAQEBAQEBAQEKg92xfpWluMiUoAAAAASUVORK5CYII=";

/// One-page PDF whose only text is "Quarterly revenue was 47 million dollars."
const DOC_PDF_B64: &str = "JVBERi0xLjQKMSAwIG9iajw8L1R5cGUvQ2F0YWxvZy9QYWdlcyAyIDAgUj4+ZW5kb2JqCjIgMCBvYmo8PC9UeXBlL1BhZ2VzL0tpZHNbMyAwIFJdL0NvdW50IDE+PmVuZG9iagozIDAgb2JqPDwvVHlwZS9QYWdlL1BhcmVudCAyIDAgUi9NZWRpYUJveFswIDAgNjEyIDc5Ml0vUmVzb3VyY2VzPDwvRm9udDw8L0YxIDUgMCBSPj4+Pi9Db250ZW50cyA0IDAgUj4+ZW5kb2JqCjQgMCBvYmo8PC9MZW5ndGggNzI+PnN0cmVhbQpCVCAvRjEgMTggVGYgNzIgNzAwIFRkIChRdWFydGVybHkgcmV2ZW51ZSB3YXMgNDcgbWlsbGlvbiBkb2xsYXJzLikgVGogRVQKZW5kc3RyZWFtZW5kb2JqCjUgMCBvYmo8PC9UeXBlL0ZvbnQvU3VidHlwZS9UeXBlMS9CYXNlRm9udC9IZWx2ZXRpY2E+PmVuZG9iagp4cmVmCjAgNgowMDAwMDAwMDAwIDY1NTM1IGYgCjAwMDAwMDAwMDkgMDAwMDAgbiAKMDAwMDAwMDA1MiAwMDAwMCBuIAowMDAwMDAwMTAxIDAwMDAwIG4gCjAwMDAwMDAyMTEgMDAwMDAgbiAKMDAwMDAwMDMyOCAwMDAwMCBuIAp0cmFpbGVyPDwvU2l6ZSA2L1Jvb3QgMSAwIFI+PgpzdGFydHhyZWYKMzg5CiUlRU9GCg==";

#[derive(Clone, Copy)]
enum Media {
    Image {
        mime: &'static str,
        b64: &'static str,
    },
    Document {
        mime: &'static str,
        b64: &'static str,
        filename: &'static str,
    },
}

impl Media {
    fn data_url(self) -> String {
        match self {
            Self::Image { mime, b64 } | Self::Document { mime, b64, .. } => {
                format!("data:{mime};base64,{b64}")
            }
        }
    }
}

struct Scenario {
    name: &'static str,
    system: Option<&'static str>,
    user: &'static str,
    tool: Option<Tool>,
    /// Attachment carried on the user turn, rendered natively per provider.
    media: Option<Media>,
    /// What a correct answer contains. Printed with every result so the human reading
    /// the report can tell "the model saw it" from "the model guessed".
    expect: Option<&'static str>,
}

fn scenarios() -> Vec<Scenario> {
    vec![
        // Low-variance free text — highlights pure provider "brain" differences.
        Scenario {
            name: "text_qa",
            system: Some("You are concise. Answer with as few words as possible."),
            user: "What is the capital of France? Answer with only the city name.",
            tool: None,
            media: None,
            expect: Some("Paris"),
        },
        // Tool call — structure (name + args) stays stable even when wording drifts,
        // and exercises the hardest parts of the translation (tool schema + tool_use).
        Scenario {
            name: "tool_call_translate",
            system: Some("You are a translation agent. Always use the translate_text tool."),
            user: "Translate 'my name is csg' into Hindi.",
            tool: Some(Tool {
                name: "translate_text",
                description: "Translate plain text into a target language.",
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "The text to translate." },
                        "target_language": { "type": "string", "description": "Language to translate into." }
                    },
                    "required": ["text", "target_language"]
                }),
                force: true,
            }),
            media: None,
            expect: Some("a translate_text call with target_language = Hindi"),
        },
        // Image → the attachment must survive the whole round trip. Two colours in a
        // fixed order: a model that never received the image cannot land both.
        Scenario {
            name: "image_describe",
            system: Some("You are concise."),
            user: "Name the colour of the top half of this image, then the bottom half. \
                   Answer with exactly two words.",
            tool: None,
            media: Some(Media::Image {
                mime: "image/png",
                b64: IMAGE_PNG_B64,
            }),
            expect: Some("blue yellow"),
        },
        // Document → same idea, and the figure appears nowhere but inside the PDF.
        Scenario {
            name: "pdf_describe",
            system: Some("You are concise."),
            user: "What revenue figure is stated in this document? \
                   Answer with just the number and unit.",
            tool: None,
            media: Some(Media::Document {
                mime: "application/pdf",
                b64: DOC_PDF_B64,
                filename: "report.pdf",
            }),
            expect: Some("47 million"),
        },
    ]
}

/// The user turn in OpenAI shape — a bare string, or a part array when media rides along.
fn openai_user_content(s: &Scenario) -> Value {
    let Some(media) = s.media else {
        return json!(s.user);
    };
    let attachment = match media {
        Media::Image { .. } => json!({
            "type": "image_url",
            "image_url": { "url": media.data_url() },
        }),
        Media::Document { filename, .. } => json!({
            "type": "file",
            "file": { "file_data": media.data_url(), "filename": filename },
        }),
    };
    json!([{ "type": "text", "text": s.user }, attachment])
}

/// The user turn in Anthropic shape — `image` / `document` blocks with a base64 source.
fn anthropic_user_content(s: &Scenario) -> Value {
    let Some(media) = s.media else {
        return json!(s.user);
    };
    let (kind, mime, b64) = match media {
        Media::Image { mime, b64 } => ("image", mime, b64),
        Media::Document { mime, b64, .. } => ("document", mime, b64),
    };
    json!([
        { "type": "text", "text": s.user },
        { "type": kind, "source": { "type": "base64", "media_type": mime, "data": b64 } },
    ])
}

/// The user turn in Gemini shape — `inlineData` siblings of the text part.
fn gemini_user_parts(s: &Scenario) -> Value {
    let mut parts = vec![json!({ "text": s.user })];
    if let Some(media) = s.media {
        let (mime, b64) = match media {
            Media::Image { mime, b64 } | Media::Document { mime, b64, .. } => (mime, b64),
        };
        parts.push(json!({ "inlineData": { "mimeType": mime, "data": b64 } }));
    }
    json!(parts)
}

/// Build the native request body for `provider` from a logical scenario.
fn native_request(provider: Provider, s: &Scenario, model: &str) -> Value {
    match provider {
        Provider::OpenAi => openai_request(s, model),
        Provider::Anthropic => anthropic_request(s, model),
        Provider::Gemini => gemini_request(s),
    }
}

fn openai_request(s: &Scenario, model: &str) -> Value {
    let mut messages = Vec::new();
    if let Some(sys) = s.system {
        messages.push(json!({ "role": "system", "content": sys }));
    }
    messages.push(json!({ "role": "user", "content": openai_user_content(s) }));

    let mut body = json!({
        "model": model,
        "messages": messages,
        "temperature": TEMPERATURE,
        "max_tokens": MAX_TOKENS,
    });
    if let Some(t) = &s.tool {
        body["tools"] = json!([{
            "type": "function",
            "function": { "name": t.name, "description": t.description, "parameters": t.parameters },
        }]);
        body["tool_choice"] = if t.force {
            json!("required")
        } else {
            json!("auto")
        };
    }
    body
}

fn anthropic_request(s: &Scenario, model: &str) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "temperature": TEMPERATURE,
        "messages": [{ "role": "user", "content": anthropic_user_content(s) }],
    });
    if let Some(sys) = s.system {
        body["system"] = json!(sys);
    }
    if let Some(t) = &s.tool {
        body["tools"] = json!([{
            "name": t.name, "description": t.description, "input_schema": t.parameters,
        }]);
        body["tool_choice"] = if t.force {
            json!({ "type": "any" })
        } else {
            json!({ "type": "auto" })
        };
    }
    body
}

/// Gemini takes the model in the URL, not the body.
fn gemini_request(s: &Scenario) -> Value {
    let mut body = json!({
        "contents": [{ "role": "user", "parts": gemini_user_parts(s) }],
        "generationConfig": { "temperature": TEMPERATURE, "maxOutputTokens": MAX_TOKENS },
    });
    if let Some(sys) = s.system {
        body["systemInstruction"] = json!({ "parts": [{ "text": sys }] });
    }
    if let Some(t) = &s.tool {
        body["tools"] = json!([{
            "functionDeclarations": [{ "name": t.name, "description": t.description, "parameters": t.parameters }],
        }]);
        body["toolConfig"] = json!({
            "functionCallingConfig": { "mode": if t.force { "ANY" } else { "AUTO" } },
        });
    }
    body
}

// ─────────────────────────────────────────────────────────────────────────────
// Calls
// ─────────────────────────────────────────────────────────────────────────────

/// Direct, un-translated call to a provider's real API (mirrors the auth each
/// provider client uses). Returns the raw native response JSON.
async fn direct_call(
    http: &reqwest::Client,
    provider: Provider,
    cfg: &GatewayConfig,
    body: &Value,
    model: &str,
    key: &str,
) -> Result<Value, String> {
    let base = provider.base(cfg);
    let req = match provider {
        Provider::OpenAi => http
            .post(format!("{base}/chat/completions"))
            .bearer_auth(key)
            .json(body),
        Provider::Anthropic => http
            .post(format!("{base}/messages"))
            .header("x-api-key", key)
            .header("anthropic-version", "2023-06-01")
            .json(body),
        Provider::Gemini => http
            .post(format!("{base}/models/{model}:generateContent"))
            .header("x-goog-api-key", key)
            .json(body),
    };

    let resp = req.send().await.map_err(|e| format!("transport: {e}"))?;
    let status = resp.status();
    let value: Value = resp.json().await.map_err(|e| format!("parse: {e}"))?;
    if !status.is_success() {
        return Err(format!("http {status}: {value}"));
    }
    Ok(value)
}

/// The router's translation path via direct composition: parse the client's X-shaped
/// body into IR, run it through provider Y, render the IR response back into X's shape.
async fn translated_call(
    http: &reqwest::Client,
    cfg: &GatewayConfig,
    client: Provider,
    backend: Provider,
    x_body: Value,
    backend_model: &str,
    backend_key: &str,
) -> Result<Value, String> {
    let inbound = inbound_for(client.format());
    let ir_req = inbound
        .parse_chat(x_body)
        .map_err(|e| format!("inbound parse ({}): {e}", client.label()))?;

    let resolved = ResolvedConfig {
        agent_id: String::new(),
        compress_enabled: false,
        provider: backend.label().to_string(),
        model: backend_model.to_string(),
        litellm_model: format!("{}/{}", backend.label(), backend_model),
        api_key: backend_key.to_string(),
        fallback_models: Vec::new(),
        temperature: Some(TEMPERATURE),
        max_tokens: Some(MAX_TOKENS),
        has_llm_config: false,
        pinned_model: None,
        tier1_model: None,
        tier2_model: None,
        tier3_model: None,
        platform_paid: true,
        custom_endpoint: None,
        is_coding_agent: false,
        is_discovered_agent: false,
    };

    let provider = provider_for(
        &resolved.provider,
        resolved.custom_endpoint.as_ref(),
        http,
        cfg,
    )
    .map_err(|e| format!("provider_for({}): {e}", backend.label()))?;
    let ir_resp = provider
        .chat(&ir_req, &resolved)
        .await
        .map_err(|e| format!("backend chat ({}): {e}", backend.label()))?;

    Ok(inbound.render_chat_response(ir_resp))
}

// ─────────────────────────────────────────────────────────────────────────────
// Result summaries (format-aware, so all three are comparable)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ToolCallView {
    name: String,
    arguments: Value,
}

#[derive(Serialize)]
struct Summary {
    text: String,
    tool_calls: Vec<ToolCallView>,
    finish_reason: Option<String>,
    usage: Value,
}

fn summarize(provider: Provider, v: &Value) -> Summary {
    match provider {
        Provider::OpenAi => summarize_openai(v),
        Provider::Anthropic => summarize_anthropic(v),
        Provider::Gemini => summarize_gemini(v),
    }
}

fn summarize_openai(v: &Value) -> Summary {
    let choice = v.get("choices").and_then(|c| c.get(0));
    let msg = choice.and_then(|c| c.get("message"));
    let text = msg
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or_default()
        .to_string();
    let mut tool_calls = Vec::new();
    if let Some(arr) = msg
        .and_then(|m| m.get("tool_calls"))
        .and_then(|t| t.as_array())
    {
        for tc in arr {
            let f = tc.get("function");
            let name = f
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string();
            // OpenAI arguments are a JSON *string* — parse for a like-for-like view.
            let arguments = f
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(Value::Null);
            tool_calls.push(ToolCallView { name, arguments });
        }
    }
    let finish_reason = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str())
        .map(String::from);
    Summary {
        text,
        tool_calls,
        finish_reason,
        usage: v.get("usage").cloned().unwrap_or(Value::Null),
    }
}

fn summarize_anthropic(v: &Value) -> Summary {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    if let Some(blocks) = v.get("content").and_then(|c| c.as_array()) {
        for block in blocks {
            match block.get("type").and_then(|t| t.as_str()) {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        text.push_str(t);
                    }
                }
                Some("tool_use") => tool_calls.push(ToolCallView {
                    name: block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    arguments: block.get("input").cloned().unwrap_or(Value::Null),
                }),
                _ => {}
            }
        }
    }
    Summary {
        text,
        tool_calls,
        finish_reason: v
            .get("stop_reason")
            .and_then(|s| s.as_str())
            .map(String::from),
        usage: v.get("usage").cloned().unwrap_or(Value::Null),
    }
}

fn summarize_gemini(v: &Value) -> Summary {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let candidate = v.get("candidates").and_then(|c| c.get(0));
    if let Some(parts) = candidate
        .and_then(|c| c.get("content"))
        .and_then(|c| c.get("parts"))
        .and_then(|p| p.as_array())
    {
        for part in parts {
            if let Some(t) = part.get("text").and_then(|t| t.as_str()) {
                text.push_str(t);
            }
            if let Some(fc) = part.get("functionCall") {
                tool_calls.push(ToolCallView {
                    name: fc
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    arguments: fc.get("args").cloned().unwrap_or(Value::Null),
                });
            }
        }
    }
    Summary {
        text,
        tool_calls,
        finish_reason: candidate
            .and_then(|c| c.get("finishReason"))
            .and_then(|f| f.as_str())
            .map(String::from),
        usage: v.get("usageMetadata").cloned().unwrap_or(Value::Null),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Report
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Serialize)]
struct ResultRecord {
    label: String,
    /// Which native format the `raw` payload (and thus `summary`) is in.
    format: String,
    ok: bool,
    error: Option<String>,
    summary: Option<Summary>,
    raw: Value,
}

impl ResultRecord {
    fn from_call(label: String, format: Provider, outcome: Result<Value, String>) -> Self {
        match outcome {
            Ok(raw) => ResultRecord {
                label,
                format: format.label().to_string(),
                ok: true,
                error: None,
                summary: Some(summarize(format, &raw)),
                raw,
            },
            Err(e) => ResultRecord {
                label,
                format: format.label().to_string(),
                ok: false,
                error: Some(e),
                summary: None,
                raw: Value::Null,
            },
        }
    }
}

#[derive(Serialize)]
struct CaseReport {
    scenario: String,
    /// What a correct answer contains — the yardstick for reading the three results.
    #[serde(skip_serializing_if = "Option::is_none")]
    expect: Option<String>,
    /// Whether this scenario carried an attachment, so a reader can tell at a glance
    /// which rows actually exercise the media path.
    media: Option<String>,
    client_format: String,
    backend_provider: String,
    pure_x: ResultRecord,
    pure_y: ResultRecord,
    translated: ResultRecord,
}

fn print_case(c: &CaseReport) {
    println!("\n══════════════════════════════════════════════════════════════════");
    println!(
        "Scenario: {}   |   client(X)={}   backend(Y)={}",
        c.scenario, c.client_format, c.backend_provider
    );
    if let Some(media) = &c.media {
        println!("Attachment: {media}");
    }
    if let Some(expect) = &c.expect {
        println!("Expected:   {expect}");
    }
    println!("──────────────────────────────────────────────────────────────────");
    print_result("PURE X  — direct call to client's provider", &c.pure_x);
    print_result("PURE Y  — direct call to backend provider", &c.pure_y);
    print_result(
        &format!(
            "X → Y   — router path (served by {}, returned in {} shape)",
            c.backend_provider, c.client_format
        ),
        &c.translated,
    );
}

fn print_result(title: &str, r: &ResultRecord) {
    println!("\n  ▸ {title}   [{}]", r.label);
    if !r.ok {
        println!("      ERROR: {}", r.error.as_deref().unwrap_or("<unknown>"));
        return;
    }
    let Some(s) = &r.summary else { return };
    if !s.text.is_empty() {
        println!("      text: {}", s.text.replace('\n', " "));
    }
    for tc in &s.tool_calls {
        // Pretty-print the tool arguments so nested JSON is readable.
        let args = serde_json::to_string_pretty(&tc.arguments)
            .unwrap_or_else(|_| tc.arguments.to_string());
        println!("      tool_call: {}", tc.name);
        for line in args.lines() {
            println!("          {line}");
        }
    }
    println!("      finish_reason: {:?}", s.finish_reason);
    println!("      usage: {}", s.usage);

    // Full raw provider payload — the actual response over the wire.
    // On by default; set NASIKO_TRANSLATION_VERBOSE=0 to suppress.
    if verbose() {
        let raw = serde_json::to_string_pretty(&r.raw).unwrap_or_else(|_| r.raw.to_string());
        println!("      raw response ({}):", r.format);
        for line in raw.lines() {
            println!("        {line}");
        }
    }
}

/// Whether to print full raw provider payloads. Defaults to on; opt out with
/// `NASIKO_TRANSLATION_VERBOSE=0` (or `false`/`no`).
fn verbose() -> bool {
    match std::env::var("NASIKO_TRANSLATION_VERBOSE") {
        Ok(v) => !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no"),
        Err(_) => true,
    }
}

fn dump_json(client: Provider, backend: Provider, cases: &[CaseReport]) {
    let dir = env_or(
        "NASIKO_TRANSLATION_REPORT_DIR",
        "target/provider_translation_report",
    );
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("could not create report dir {dir}: {e}");
        return;
    }
    let path = PathBuf::from(&dir).join(format!("{}_to_{}.json", client.label(), backend.label()));
    let payload = json!({
        "client_provider": client.label(),
        "backend_provider": backend.label(),
        "temperature": TEMPERATURE,
        "max_tokens": MAX_TOKENS,
        "cases": cases,
    });
    match serde_json::to_string_pretty(&payload) {
        Ok(text) => match fs::write(&path, text) {
            Ok(()) => println!("\n📄 report written: {}", path.display()),
            Err(e) => eprintln!("could not write {}: {e}", path.display()),
        },
        Err(e) => eprintln!("could not serialize report: {e}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Runner
// ─────────────────────────────────────────────────────────────────────────────

/// For one directed pair (client X ≠ backend Y): over every scenario, produce the
/// three results, print them, and dump the JSON report. Report-only — always passes.
async fn run_pair(client: Provider, backend: Provider) {
    assert!(
        client != backend,
        "translation comparison only makes sense when client format differs from backend"
    );

    let (Some(x_key), Some(y_key)) = (client.api_key(), backend.api_key()) else {
        eprintln!(
            "SKIP {} → {}: set both {} and {} to run this comparison",
            client.label(),
            backend.label(),
            client.key_env(),
            backend.key_env(),
        );
        return;
    };

    let cfg = GatewayConfig::from_env();
    let http = reqwest::Client::new();
    let x_model = client.model();
    let y_model = backend.model();

    println!(
        "\n#### {} ({}) → {} ({}) ####",
        client.label(),
        x_model,
        backend.label(),
        y_model
    );

    let mut cases = Vec::new();
    for s in scenarios() {
        let pure_x = ResultRecord::from_call(
            format!("pure_{}", client.label()),
            client,
            direct_call(
                &http,
                client,
                &cfg,
                &native_request(client, &s, &x_model),
                &x_model,
                &x_key,
            )
            .await,
        );

        let pure_y = ResultRecord::from_call(
            format!("pure_{}", backend.label()),
            backend,
            direct_call(
                &http,
                backend,
                &cfg,
                &native_request(backend, &s, &y_model),
                &y_model,
                &y_key,
            )
            .await,
        );

        let translated = ResultRecord::from_call(
            format!("translated_{}_to_{}", client.label(), backend.label()),
            client, // rendered back into the client's (X) shape
            translated_call(
                &http,
                &cfg,
                client,
                backend,
                native_request(client, &s, &x_model),
                &y_model,
                &y_key,
            )
            .await,
        );

        let case = CaseReport {
            scenario: s.name.to_string(),
            expect: s.expect.map(str::to_string),
            media: s.media.map(|m| match m {
                Media::Image { mime, b64 } => format!("image ({mime}, {} b64 chars)", b64.len()),
                Media::Document { mime, b64, .. } => {
                    format!("document ({mime}, {} b64 chars)", b64.len())
                }
            }),
            client_format: client.label().to_string(),
            backend_provider: backend.label().to_string(),
            pure_x,
            pure_y,
            translated,
        };
        print_case(&case);
        cases.push(case);
    }

    dump_json(client, backend, &cases);
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
// The six directed pairs (X ≠ Y). Each yields pure-X / pure-Y / X→Y per scenario.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "live: needs OPENAI_API_KEY + ANTHROPIC_API_KEY"]
async fn openai_to_anthropic() {
    run_pair(Provider::OpenAi, Provider::Anthropic).await;
}

#[tokio::test]
#[ignore = "live: needs OPENAI_API_KEY + GEMINI_API_KEY"]
async fn openai_to_gemini() {
    run_pair(Provider::OpenAi, Provider::Gemini).await;
}

#[tokio::test]
#[ignore = "live: needs ANTHROPIC_API_KEY + OPENAI_API_KEY"]
async fn anthropic_to_openai() {
    run_pair(Provider::Anthropic, Provider::OpenAi).await;
}

#[tokio::test]
#[ignore = "live: needs ANTHROPIC_API_KEY + GEMINI_API_KEY"]
async fn anthropic_to_gemini() {
    run_pair(Provider::Anthropic, Provider::Gemini).await;
}

#[tokio::test]
#[ignore = "live: needs GEMINI_API_KEY + OPENAI_API_KEY"]
async fn gemini_to_openai() {
    run_pair(Provider::Gemini, Provider::OpenAi).await;
}

#[tokio::test]
#[ignore = "live: needs GEMINI_API_KEY + ANTHROPIC_API_KEY"]
async fn gemini_to_anthropic() {
    run_pair(Provider::Gemini, Provider::Anthropic).await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Harness self-check — hermetic, always runs
// ─────────────────────────────────────────────────────────────────────────────

/// Guards the harness itself. The live pairs above cost money and need three keys, so a
/// scenario that silently fails to attach its media would be discovered late and
/// expensively — and would look like a *translation* bug rather than a test-rig one.
/// This asserts the attachment reaches every dialect before anyone spends a cent.
#[test]
fn media_scenarios_attach_in_every_dialect() {
    let media: Vec<Scenario> = scenarios()
        .into_iter()
        .filter(|s| s.media.is_some())
        .collect();
    assert_eq!(media.len(), 2, "expected an image and a document scenario");

    for s in &media {
        // OpenAI: a part array carrying text + the attachment.
        let oa = openai_request(s, "gpt-4o");
        let parts = oa["messages"]
            .as_array()
            .and_then(|m| m.last())
            .and_then(|m| m["content"].as_array())
            .unwrap_or_else(|| panic!("{}: openai content is not a part array", s.name));
        assert_eq!(parts.len(), 2, "{}: openai", s.name);
        let attached = &parts[1];
        let url = attached["image_url"]["url"]
            .as_str()
            .or_else(|| attached["file"]["file_data"].as_str())
            .unwrap_or_else(|| panic!("{}: openai attachment has no payload", s.name));
        assert!(
            url.starts_with("data:"),
            "{}: openai not a data URL",
            s.name
        );

        // Anthropic: an image/document block with a base64 source.
        let an = anthropic_request(s, "claude-3-5-sonnet-20241022");
        let blocks = an["messages"][0]["content"]
            .as_array()
            .unwrap_or_else(|| panic!("{}: anthropic content is not a block array", s.name));
        assert_eq!(blocks.len(), 2, "{}: anthropic", s.name);
        assert_eq!(
            blocks[1]["source"]["type"], "base64",
            "{}: anthropic",
            s.name
        );
        assert!(
            !blocks[1]["source"]["data"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "{}: anthropic payload empty",
            s.name
        );

        // Gemini: inlineData sitting beside the text part.
        let ge = gemini_request(s);
        let gparts = ge["contents"][0]["parts"]
            .as_array()
            .unwrap_or_else(|| panic!("{}: gemini parts missing", s.name));
        assert_eq!(gparts.len(), 2, "{}: gemini", s.name);
        assert!(
            !gparts[1]["inlineData"]["data"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "{}: gemini payload empty",
            s.name
        );
    }
}

/// The fixtures must stay real files. If either is truncated or re-encoded badly the
/// live run fails upstream with a provider-side parse error that looks like our bug.
#[test]
fn media_fixtures_are_well_formed() {
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD
        .decode(IMAGE_PNG_B64)
        .expect("image fixture is not valid base64");
    assert_eq!(
        &png[..8],
        b"\x89PNG\r\n\x1a\n",
        "image fixture is not a PNG"
    );

    let pdf = base64::engine::general_purpose::STANDARD
        .decode(DOC_PDF_B64)
        .expect("pdf fixture is not valid base64");
    assert_eq!(&pdf[..5], b"%PDF-", "pdf fixture is not a PDF");
    assert!(
        pdf.windows(7)
            .any(|w| w == b"%%EOF\n".as_slice() || w == b"%%EOF\r\n".as_slice())
            || pdf.ends_with(b"%%EOF\n"),
        "pdf fixture has no EOF marker"
    );
    // The answer the live scenario checks for must actually be in the document.
    assert!(
        String::from_utf8_lossy(&pdf).contains("47 million"),
        "pdf fixture no longer states the expected figure"
    );
}
