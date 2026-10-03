//! Measurement: full-request token counts for three ways of presenting the same tools, the
//! bypass rate, round-trip fidelity, local encode/decode time and (optionally) live adherence.
//!
//! Pure with respect to tokenizers and TOON: both arrive as trait objects / closures from the
//! example binary, so this module is testable with a fake counter and carries no heavy
//! dependencies into the library.

use std::time::Instant;

use nasiko_llm_router::compact_tools::{self, Compiled, Plan};
use nasiko_llm_router::config::GatewayConfig;
use nasiko_llm_router::inbound::InboundFormat;
use nasiko_llm_router::ir::{ChatRequest, Message};
use nasiko_tool_compact::{ToolDef, decode_calls, encode_tools};
use serde_json::{Map, Value, json};

use super::eval::{
    Case, CaseInputs, ExpectedCall, native_body, parse_request, render_calls, tool_defs,
};

/// Counts tokens of a text. The example binary supplies `o200k_base`; tests use a fake.
pub trait TokenCounter {
    fn count(&self, text: &str) -> usize;
}

/// Encodes one JSON value as TOON text. Supplied by the example binary.
pub type ToonEncoder<'a> = &'a dyn Fn(&Value) -> Result<String, String>;

/// The TOON variant's own one-line header. The call protocol that follows the definitions is
/// the compact notation's own `INSTRUCTIONS`, verbatim, so the comparison isolates the
/// definitions encoding and gives each format only the explanation it needs.
pub const TOON_HEADER: &str = "Tools, one TOON (Token-Oriented Object Notation) document per tool with name, description and JSON Schema parameters:";

pub struct Variants {
    pub native: Value,
    pub toon: Option<Value>,
    pub toon_error: Option<String>,
    pub compact: Option<Value>,
    pub bypass: Option<&'static str>,
    pub compiled: Option<Compiled>,
    pub tool_defs: Vec<ToolDef>,
}

fn eval_config() -> GatewayConfig {
    GatewayConfig {
        compact_tools_enabled: true,
        ..GatewayConfig::default()
    }
}

/// Insert one system message after the leading system run, the same placement `apply` uses.
fn insert_catalog_message(req: &mut ChatRequest, text: String) {
    req.tools = None;
    req.tool_choice = None;
    req.extra.remove("parallel_tool_calls");
    let index = req
        .messages
        .iter()
        .take_while(|m| m.role == "system")
        .count();
    req.messages.insert(
        index,
        Message {
            role: "system".into(),
            content: Some(Value::String(text)),
            name: None,
            tool_calls: None,
            tool_call_id: None,
            extra: Map::new(),
        },
    );
}

/// Build the native, TOON and compact request bodies for one case. TOON and compact exist only
/// when the request is eligible for compaction, so a bypass counts as zero saving for both.
pub fn build_variants(
    inputs: CaseInputs<'_>,
    catalog: &[Value],
    model: &str,
    max_output_tokens: u32,
    toon: Option<ToonEncoder<'_>>,
) -> Result<Variants, String> {
    let (body, tools) = native_body(inputs, catalog, model, max_output_tokens)?;
    let defs = tool_defs(&tools);
    let req = parse_request(&body)?;
    let native = serde_json::to_value(&req).map_err(|e| e.to_string())?;
    let plan = compact_tools::plan_from_body(&req, &body, InboundFormat::OpenAi, &eval_config());
    let (compiled, bypass) = match plan {
        Plan::Apply(c) => (Some(c), None),
        Plan::Bypass(b) => (None, Some(b.as_label())),
    };
    let compact = compiled.as_ref().map(|c| {
        let mut r = req.clone();
        compact_tools::apply(&mut r, c);
        serde_json::to_value(&r).unwrap_or(Value::Null)
    });
    let (toon_body, toon_error) = match (compiled.as_ref(), toon) {
        (Some(_), Some(encode)) => {
            let mut docs = Vec::with_capacity(tools.len());
            let mut error = None;
            for t in &tools {
                match encode(&t["function"]) {
                    Ok(doc) => docs.push(doc),
                    Err(e) => {
                        error = Some(e);
                        break;
                    }
                }
            }
            match error {
                None => {
                    let mut r = req.clone();
                    insert_catalog_message(
                        &mut r,
                        format!(
                            "{TOON_HEADER}\n{}\n{}",
                            docs.join("\n\n"),
                            nasiko_tool_compact::INSTRUCTIONS
                        ),
                    );
                    (Some(serde_json::to_value(&r).unwrap_or(Value::Null)), None)
                }
                Some(e) => (None, Some(e)),
            }
        }
        _ => (None, None),
    };
    Ok(Variants {
        native,
        toon: toon_body,
        toon_error,
        compact,
        bypass,
        compiled,
        tool_defs: defs,
    })
}

/// What one live attempt of one variant produced. Only `Matched` counts as correct; the
/// denominator of the adherence rate is every attempted case (matched, mismatched and output
/// failures). Transport errors and skips are reported beside it, never folded into either number.
#[derive(Debug, Clone, PartialEq)]
pub enum VariantLive {
    /// The reply finished and the released calls equal the expected ones under the case's rules.
    Matched,
    /// The reply finished and released calls, but not the expected ones.
    Mismatched,
    /// The model's output could not be used: decoding, validation or an unfinished completion.
    /// Carries the error kind.
    OutputFailure(String),
    /// The request never produced a reply to judge (HTTP or connection failure).
    TransportError(String),
    /// Not attempted (after an authentication failure stopped the run).
    Skipped,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveAdherence {
    pub native: Option<VariantLive>,
    pub toon: Option<VariantLive>,
    pub compact: Option<VariantLive>,
}

#[derive(Debug, Clone)]
pub struct CaseMeasurement {
    pub id: String,
    pub source: String,
    pub bypass: Option<&'static str>,
    pub native_tokens: usize,
    pub toon_tokens: Option<usize>,
    pub toon_error: Option<String>,
    pub compact_tokens: Option<usize>,
    /// Serialized bytes of the native `tools` array and of the compact system message, when
    /// the request was compacted. Bytes, not tokens: this is the quantity the router's
    /// `no_byte_saving` gate compares.
    pub definition_bytes: Option<(usize, usize)>,
    pub roundtrip_equal: Option<bool>,
    pub encode_us: Option<u128>,
    pub decode_us: Option<u128>,
    pub live: Option<LiveAdherence>,
}

pub fn count_body(counter: &dyn TokenCounter, body: &Value) -> usize {
    counter.count(&serde_json::to_string(body).unwrap_or_default())
}

/// Compare decoded calls with the expected ones: same names in order; arguments equal, except
/// that fields listed in `match.free_text_fields` only need to be present with the same type.
pub fn calls_match(
    expected: &[ExpectedCall],
    actual: &[(String, Value)],
    rules: Option<&Value>,
) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let free: Vec<&str> = rules
        .and_then(|r| r.get("free_text_fields"))
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    expected.iter().zip(actual).all(|(e, (name, args))| {
        if &e.name != name {
            return false;
        }
        let (Some(ea), Some(aa)) = (e.arguments.as_object(), args.as_object()) else {
            return e.arguments == *args;
        };
        if ea.len() != aa.len() {
            return false;
        }
        ea.iter().all(|(k, ev)| match aa.get(k) {
            None => false,
            Some(av) if free.contains(&k.as_str()) => same_type(ev, av),
            Some(av) => ev == av,
        })
    })
}

fn same_type(a: &Value, b: &Value) -> bool {
    std::mem::discriminant(a) == std::mem::discriminant(b)
}

/// Measure one case offline.
pub fn measure_case(
    case: &Case,
    source: &str,
    catalog: &[Value],
    model: &str,
    max_output_tokens: u32,
    counter: &dyn TokenCounter,
    toon: Option<ToonEncoder<'_>>,
) -> Result<CaseMeasurement, String> {
    let inputs = CaseInputs {
        tools: &case.tools,
        messages: &case.messages,
    };
    let variants = build_variants(inputs, catalog, model, max_output_tokens, toon)?;
    let native_tokens = count_body(counter, &variants.native);
    let toon_tokens = variants.toon.as_ref().map(|b| count_body(counter, b));
    let compact_tokens = variants.compact.as_ref().map(|b| count_body(counter, b));

    let (roundtrip_equal, encode_us, decode_us) = if variants.compiled.is_some() {
        let started = Instant::now();
        let encoded = encode_tools(&variants.tool_defs);
        let encode_us = started.elapsed().as_micros();
        let rendered = render_calls(&case.expected);
        let started = Instant::now();
        let decoded = decode_calls(&rendered, &variants.tool_defs);
        let decode_us = started.elapsed().as_micros();
        let equal = encoded.is_ok()
            && decoded.is_ok_and(|d| {
                let actual: Vec<(String, Value)> =
                    d.calls.into_iter().map(|c| (c.name, c.arguments)).collect();
                calls_match(&case.expected, &actual, None)
            });
        (Some(equal), Some(encode_us), Some(decode_us))
    } else {
        (None, None, None)
    };

    Ok(CaseMeasurement {
        id: case.id.clone(),
        source: source.to_owned(),
        bypass: variants.bypass,
        native_tokens,
        toon_tokens,
        toon_error: variants.toon_error,
        compact_tokens,
        definition_bytes: variants
            .compiled
            .as_ref()
            .map(|c| (c.definitions_bytes_in, c.definitions_bytes_out)),
        roundtrip_equal,
        encode_us,
        decode_us,
        live: None,
    })
}

pub struct Report {
    pub cases: Vec<CaseMeasurement>,
    pub tokenizer: String,
    pub toon_note: String,
    pub live_model: Option<String>,
    pub live_endpoint: Option<String>,
}

/// Token reduction of `variant` relative to `native`: `100 * (1 - variant / native)`, negative
/// when the variant is larger.
pub fn pct(native: usize, variant: usize) -> String {
    if native == 0 {
        return "n/a".into();
    }
    format!("{:+.1}%", 100.0 * (1.0 - variant as f64 / native as f64))
}

fn median(mut v: Vec<u128>) -> u128 {
    if v.is_empty() {
        return 0;
    }
    v.sort_unstable();
    v[v.len() / 2]
}

/// Live correctness for one variant: `matched / attempted`, where attempted counts every case
/// whose request produced a reply (matched, mismatched, or an output the rules rejected).
pub fn adherence(
    cases: &[&CaseMeasurement],
    pick: fn(&LiveAdherence) -> Option<&VariantLive>,
) -> String {
    let (mut matched, mut mismatched, mut output_failures, mut transport, mut skipped) =
        (0, 0, 0, 0, 0);
    for c in cases {
        match c.live.as_ref().and_then(pick) {
            None => {}
            Some(VariantLive::Matched) => matched += 1,
            Some(VariantLive::Mismatched) => mismatched += 1,
            Some(VariantLive::OutputFailure(_)) => output_failures += 1,
            Some(VariantLive::TransportError(_)) => transport += 1,
            Some(VariantLive::Skipped) => skipped += 1,
        }
    }
    let attempted = matched + mismatched + output_failures;
    if attempted + transport + skipped == 0 {
        return "not run".into();
    }
    format!(
        "{matched}/{attempted} matched ({mismatched} mismatched, {output_failures} output failures; \
{transport} transport errors, {skipped} skipped)"
    )
}

/// Render the report. Every number is an output of this run; nothing is estimated.
pub fn render_markdown(report: &Report) -> String {
    let mut out = String::new();
    out.push_str("# Compact tool definitions: measurement report\n\n");
    out.push_str("## What is measured\n\n");
    out.push_str(&format!(
        "- Tokens: `{}` count of the full serialized chat-completions request body as the router \
would send it (messages, definitions or tools, instructions, every other field). This is a proxy for \
billed prompt tokens, not dollars or latency; providers serialize native `tools` into their own prompt \
form, which is not public.\n",
        report.tokenizer
    ));
    out.push_str(&format!("- TOON: {}\n", report.toon_note));
    out.push_str("- A bypassed case has no TOON or compact variant and counts as zero saving for both. Negative figures mean the variant was larger than native.\n");
    out.push_str("- Round trip: the case's expected calls rendered in the call grammar and decoded against the original schemas.\n");
    out.push_str("- Timing: wall-clock microseconds for `encode_tools` and `decode_calls` on this machine, single run.\n");
    match (&report.live_model, &report.live_endpoint) {
        (Some(model), Some(endpoint)) => out.push_str(&format!(
            "- Live adherence: model `{model}` at `{endpoint}`, temperature 0, same output cap, one request at a time; a variant \"matches\" when the production finalization rules release calls equal to the expected ones (free-text fields by presence and type).\n"
        )),
        _ => out.push_str("- Live adherence: not run (set PROVIDER_BASE_URL and MODEL).\n"),
    }

    let mut sources: Vec<&str> = report.cases.iter().map(|c| c.source.as_str()).collect();
    sources.sort_unstable();
    sources.dedup();
    for source in sources {
        let cases: Vec<&CaseMeasurement> =
            report.cases.iter().filter(|c| c.source == source).collect();
        out.push_str(&format!("\n## {source}\n\n"));
        let native: usize = cases.iter().map(|c| c.native_tokens).sum();
        let toon: usize = cases
            .iter()
            .map(|c| c.toon_tokens.unwrap_or(c.native_tokens))
            .sum();
        let compact: usize = cases
            .iter()
            .map(|c| c.compact_tokens.unwrap_or(c.native_tokens))
            .sum();
        let bypassed = cases.iter().filter(|c| c.bypass.is_some()).count();
        out.push_str(&format!(
            "Cases: {} ({} bypassed)\n\n",
            cases.len(),
            bypassed
        ));
        out.push_str("| Format | Tokens (sum) | Reduction vs native |\n|---|---:|---:|\n");
        out.push_str(&format!("| native | {native} | +0.0% |\n"));
        out.push_str(&format!("| TOON | {toon} | {} |\n", pct(native, toon)));
        out.push_str(&format!(
            "| compact | {compact} | {} |\n\n",
            pct(native, compact)
        ));

        out.push_str("| Case | Native | TOON | Compact | Compact Δ | Def. bytes in→out | Bypass | Round trip | encode µs | decode µs |\n|---|---:|---:|---:|---:|---:|---|---|---:|---:|\n");
        for c in &cases {
            let toon = c
                .toon_tokens
                .map_or_else(|| "-".to_owned(), |t| t.to_string());
            let compact = c
                .compact_tokens
                .map_or_else(|| "-".to_owned(), |t| t.to_string());
            let delta = c
                .compact_tokens
                .map_or_else(|| "+0.0%".to_owned(), |t| pct(c.native_tokens, t));
            let rt = match c.roundtrip_equal {
                Some(true) => "ok",
                Some(false) => "MISMATCH",
                None => "-",
            };
            let bytes = c
                .definition_bytes
                .map_or_else(|| "-".to_owned(), |(i, o)| format!("{i}→{o}"));
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                c.id,
                c.native_tokens,
                toon,
                compact,
                delta,
                bytes,
                c.bypass.unwrap_or("-"),
                rt,
                c.encode_us
                    .map_or_else(|| "-".to_owned(), |u| u.to_string()),
                c.decode_us
                    .map_or_else(|| "-".to_owned(), |u| u.to_string()),
            ));
        }

        let mut histogram: Vec<(&str, usize)> = Vec::new();
        for c in &cases {
            if let Some(b) = c.bypass {
                match histogram.iter_mut().find(|(k, _)| *k == b) {
                    Some((_, n)) => *n += 1,
                    None => histogram.push((b, 1)),
                }
            }
        }
        out.push_str("\nBypass reasons: ");
        if histogram.is_empty() {
            out.push_str("none\n");
        } else {
            let parts: Vec<String> = histogram.iter().map(|(k, n)| format!("{k}={n}")).collect();
            out.push_str(&parts.join(", "));
            out.push('\n');
        }
        let toon_errors: Vec<String> = cases
            .iter()
            .filter_map(|c| c.toon_error.as_ref().map(|e| format!("{}: {e}", c.id)))
            .collect();
        if !toon_errors.is_empty() {
            out.push_str(&format!(
                "TOON encoding failures: {}\n",
                toon_errors.join("; ")
            ));
        }

        let enc: Vec<u128> = cases.iter().filter_map(|c| c.encode_us).collect();
        let dec: Vec<u128> = cases.iter().filter_map(|c| c.decode_us).collect();
        if !enc.is_empty() {
            out.push_str(&format!(
                "Encode µs min/median/max: {}/{}/{}; decode µs min/median/max: {}/{}/{}\n",
                enc.iter().min().unwrap_or(&0),
                median(enc.clone()),
                enc.iter().max().unwrap_or(&0),
                dec.iter().min().unwrap_or(&0),
                median(dec.clone()),
                dec.iter().max().unwrap_or(&0),
            ));
        }
        out.push_str(&format!(
            "Live adherence: native {}; TOON {}; compact {}\n",
            adherence(&cases, |l| l.native.as_ref()),
            adherence(&cases, |l| l.toon.as_ref()),
            adherence(&cases, |l| l.compact.as_ref()),
        ));
        let live_rows: Vec<String> = cases
            .iter()
            .filter_map(|c| c.live.as_ref().map(|l| (c, l)))
            .map(|(c, l)| {
                let cell = |v: &Option<VariantLive>| match v {
                    None => "-".to_owned(),
                    Some(VariantLive::Matched) => "match".to_owned(),
                    Some(VariantLive::Mismatched) => "mismatch".to_owned(),
                    Some(VariantLive::OutputFailure(e)) => format!("output failure ({e})"),
                    Some(VariantLive::TransportError(e)) => format!("transport error ({e})"),
                    Some(VariantLive::Skipped) => "skipped".to_owned(),
                };
                format!(
                    "| {} | {} | {} | {} |",
                    c.id,
                    cell(&l.native),
                    cell(&l.toon),
                    cell(&l.compact)
                )
            })
            .collect();
        if !live_rows.is_empty() {
            out.push_str("\n| Case | native | TOON | compact |\n|---|---|---|---|\n");
            out.push_str(&live_rows.join("\n"));
            out.push('\n');
        }
    }
    out.push_str("\n## Caveats\n\n");
    out.push_str("- Token counts are of the request JSON string; providers may tokenize native tool schemas differently inside their own prompt templates.\n");
    out.push_str("- Live adherence at temperature 0 is still a sample, not a guarantee, and reflects the specific model and endpoint named above.\n");
    out.push_str(
        "- Call ids are omitted from every comparison; the compact path assigns fresh ones.\n",
    );
    out
}

/// Turn a judged reply into a verdict: `Ok(calls)` is compared with the expected calls, `Err`
/// (decoding, validation or an unfinished completion) is an output failure.
pub fn verdict(
    judged: Result<Vec<(String, Value)>, String>,
    expected: &[ExpectedCall],
    rules: Option<&Value>,
) -> VariantLive {
    match judged {
        Ok(actual) if calls_match(expected, &actual, rules) => VariantLive::Matched,
        Ok(_) => VariantLive::Mismatched,
        Err(e) => VariantLive::OutputFailure(e),
    }
}

/// Attach a live verdict for one variant of one case.
pub fn record_live(measurement: &mut CaseMeasurement, variant: &str, verdict: VariantLive) {
    let live = measurement.live.get_or_insert_with(LiveAdherence::default);
    match variant {
        "native" => live.native = Some(verdict),
        "toon" => live.toon = Some(verdict),
        _ => live.compact = Some(verdict),
    }
}

/// Convenience for JSON output of a measurement.
pub fn measurement_json(c: &CaseMeasurement) -> Value {
    json!({
        "id": c.id, "source": c.source, "bypass": c.bypass,
        "native_tokens": c.native_tokens, "toon_tokens": c.toon_tokens, "compact_tokens": c.compact_tokens,
        "roundtrip_equal": c.roundtrip_equal, "encode_us": c.encode_us, "decode_us": c.decode_us,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Whitespace;
    impl TokenCounter for Whitespace {
        fn count(&self, text: &str) -> usize {
            text.split_whitespace().count()
        }
    }

    fn catalog() -> Vec<Value> {
        vec![
            json!({"type": "function", "function": {"name": "get_weather", "description": "Current weather for a named city, with an optional unit",
                "parameters": {"type": "object", "properties": {"city": {"type": "string", "description": "City name"}, "unit": {"type": "string", "enum": ["c", "f"], "description": "Temperature unit"}}, "required": ["city"]}}}),
            json!({"type": "function", "function": {"name": "get_forecast", "description": "Five-day forecast for a named city",
                "parameters": {"type": "object", "properties": {"city": {"type": "string", "description": "City name"}, "days": {"type": "integer", "minimum": 1, "maximum": 5, "description": "Number of days"}}, "required": ["city"]}}}),
            json!({"type": "function", "function": {"name": "strict_tool", "strict": true, "parameters": {"type": "object", "properties": {"x": {"type": "string"}}}}}),
        ]
    }

    fn case(id: &str, tools: &[&str], expected: Vec<ExpectedCall>) -> Case {
        serde_json::from_value(json!({
            "id": id, "tools": tools, "messages": [{"role": "user", "content": "Weather in Paris in celsius"}],
            "expected": expected.iter().map(|e| json!({"name": e.name, "arguments": e.arguments})).collect::<Vec<_>>()
        }))
        .unwrap()
    }

    fn fake_toon(v: &Value) -> Result<String, String> {
        Ok(format!("TOON:{}", v["name"].as_str().unwrap_or("")))
    }

    #[test]
    fn variants_share_placement_and_differ_only_in_the_definitions() {
        let c = case("a", &["get_weather", "get_forecast"], vec![]);
        let v = build_variants(
            CaseInputs {
                tools: &c.tools,
                messages: &c.messages,
            },
            &catalog(),
            "m",
            100,
            Some(&fake_toon),
        )
        .unwrap();
        assert!(v.bypass.is_none(), "{:?}", v.bypass);
        assert!(v.native["tools"].is_array());
        let toon = v.toon.unwrap();
        let compact = v.compact.unwrap();
        assert!(toon.get("tools").is_none() && compact.get("tools").is_none());
        assert_eq!(toon["messages"].as_array().unwrap().len(), 3);
        assert_eq!(compact["messages"].as_array().unwrap().len(), 3);
        assert_eq!(toon["messages"][0], compact["messages"][0]);
        assert_eq!(toon["messages"][2], compact["messages"][2]);
        let toon_text = toon["messages"][1]["content"].as_str().unwrap();
        let compact_text = compact["messages"][1]["content"].as_str().unwrap();
        assert!(toon_text.starts_with(TOON_HEADER) && toon_text.contains("\nTOON:get_weather\n"));
        assert!(compact_text.starts_with(nasiko_tool_compact::HEADER));
        // The shared call protocol is identical text at the end of both blocks.
        assert!(toon_text.ends_with(nasiko_tool_compact::INSTRUCTIONS));
        assert!(compact_text.ends_with(nasiko_tool_compact::INSTRUCTIONS));
    }

    #[test]
    fn bypassed_cases_have_no_variants_and_count_as_native() {
        let c = case("s", &["strict_tool"], vec![]);
        let m = measure_case(
            &c,
            "dev",
            &catalog(),
            "m",
            100,
            &Whitespace,
            Some(&fake_toon),
        )
        .unwrap();
        assert_eq!(m.bypass, Some("strict_or_unknown_tool_keys"));
        assert!(
            m.toon_tokens.is_none() && m.compact_tokens.is_none() && m.roundtrip_equal.is_none()
        );
        let report = Report {
            cases: vec![m],
            tokenizer: "ws".into(),
            toon_note: "fake".into(),
            live_model: None,
            live_endpoint: None,
        };
        let md = render_markdown(&report);
        assert!(md.contains("| TOON | ") && md.contains("| +0.0% |"), "{md}");
        assert!(md.contains("Bypass reasons: strict_or_unknown_tool_keys=1"));
    }

    #[test]
    fn aggregate_reduction_and_roundtrip_are_reported_with_sign() {
        let expected = vec![ExpectedCall {
            name: "get_weather".into(),
            arguments: json!({"city": "Paris", "unit": "c"}),
        }];
        let c = case("w", &["get_weather", "get_forecast"], expected);
        let m = measure_case(
            &c,
            "dev",
            &catalog(),
            "m",
            100,
            &Whitespace,
            Some(&fake_toon),
        )
        .unwrap();
        assert_eq!(m.roundtrip_equal, Some(true));
        assert!(m.compact_tokens.unwrap() > 0 && m.native_tokens > 0);
        let report = Report {
            cases: vec![m.clone()],
            tokenizer: "ws".into(),
            toon_note: "fake".into(),
            live_model: None,
            live_endpoint: None,
        };
        let md = render_markdown(&report);
        let expected_pct = pct(m.native_tokens, m.compact_tokens.unwrap());
        assert!(expected_pct.starts_with('+') || expected_pct.starts_with('-'));
        assert!(
            md.contains(&format!(
                "| compact | {} | {expected_pct} |",
                m.compact_tokens.unwrap()
            )),
            "{md}"
        );
        assert!(md.contains("| w | "));
        assert!(md.contains("Live adherence: native not run; TOON not run; compact not run"));
    }

    #[test]
    fn calls_match_respects_order_free_text_and_types() {
        let expected = vec![ExpectedCall {
            name: "send_email".into(),
            arguments: json!({"to": ["a"], "subject": "Build status", "body": "green"}),
        }];
        let rules = json!({"free_text_fields": ["subject", "body"]});
        let actual = vec![(
            "send_email".to_owned(),
            json!({"to": ["a"], "subject": "anything", "body": "else"}),
        )];
        assert!(calls_match(&expected, &actual, Some(&rules)));
        assert!(!calls_match(&expected, &actual, None));
        let wrong_type = vec![(
            "send_email".to_owned(),
            json!({"to": ["a"], "subject": 1, "body": "x"}),
        )];
        assert!(!calls_match(&expected, &wrong_type, Some(&rules)));
        let extra = vec![(
            "send_email".to_owned(),
            json!({"to": ["a"], "subject": "s", "body": "b", "cc": []}),
        )];
        assert!(!calls_match(&expected, &extra, Some(&rules)));
        assert!(!calls_match(&expected, &[], None));
        let two = vec![expected[0].clone(), expected[0].clone()];
        let swapped = vec![("other".to_owned(), json!({})), actual[0].clone()];
        assert!(!calls_match(&two, &swapped, Some(&rules)));
    }

    #[test]
    fn live_verdicts_are_recorded_per_variant_and_the_denominator_is_every_attempt() {
        let expected = vec![ExpectedCall {
            name: "get_weather".into(),
            arguments: json!({"city": "Paris"}),
        }];
        let c = case("w", &["get_weather", "get_forecast"], expected.clone());
        let mut m = measure_case(&c, "dev", &catalog(), "m", 100, &Whitespace, None).unwrap();
        record_live(
            &mut m,
            "native",
            verdict(
                Ok(vec![("get_weather".into(), json!({"city": "Paris"}))]),
                &expected,
                None,
            ),
        );
        record_live(
            &mut m,
            "compact",
            verdict(Err("incomplete_completion".into()), &expected, None),
        );
        record_live(
            &mut m,
            "toon",
            VariantLive::TransportError("http 500".into()),
        );
        let live = m.live.clone().unwrap();
        assert_eq!(live.native, Some(VariantLive::Matched));
        assert_eq!(
            live.compact,
            Some(VariantLive::OutputFailure("incomplete_completion".into()))
        );
        let report = Report {
            cases: vec![m],
            tokenizer: "ws".into(),
            toon_note: "fake".into(),
            live_model: Some("m".into()),
            live_endpoint: Some("http://e".into()),
        };
        let md = render_markdown(&report);
        assert!(md.contains(
            "native 1/1 matched (0 mismatched, 0 output failures; 0 transport errors, 0 skipped)"
        ));
        assert!(md.contains(
            "compact 0/1 matched (0 mismatched, 1 output failures; 0 transport errors, 0 skipped)"
        ));
        assert!(md.contains(
            "TOON 0/0 matched (0 mismatched, 0 output failures; 1 transport errors, 0 skipped)"
        ));
        assert!(md.contains(
            "| w | match | transport error (http 500) | output failure (incomplete_completion) |"
        ));
    }

    #[test]
    fn one_match_and_nine_decoding_errors_is_one_in_ten() {
        let expected = vec![ExpectedCall {
            name: "get_weather".into(),
            arguments: json!({"city": "Paris"}),
        }];
        let mut cases = Vec::new();
        for i in 0..10 {
            let c = case(
                &format!("c{i}"),
                &["get_weather", "get_forecast"],
                expected.clone(),
            );
            let mut m = measure_case(&c, "dev", &catalog(), "m", 100, &Whitespace, None).unwrap();
            let judged = if i == 0 {
                Ok(vec![("get_weather".into(), json!({"city": "Paris"}))])
            } else {
                Err("malformed_call".into())
            };
            record_live(&mut m, "compact", verdict(judged, &expected, None));
            cases.push(m);
        }
        // Two more cases never produced a reply: they are reported beside the rate, not inside it.
        let c = case("t", &["get_weather", "get_forecast"], expected.clone());
        let mut m = measure_case(&c, "dev", &catalog(), "m", 100, &Whitespace, None).unwrap();
        record_live(
            &mut m,
            "compact",
            VariantLive::TransportError("timeout".into()),
        );
        cases.push(m);
        let c = case("s", &["get_weather", "get_forecast"], expected.clone());
        let mut m = measure_case(&c, "dev", &catalog(), "m", 100, &Whitespace, None).unwrap();
        record_live(&mut m, "compact", VariantLive::Skipped);
        cases.push(m);
        let refs: Vec<&CaseMeasurement> = cases.iter().collect();
        assert_eq!(
            adherence(&refs, |l| l.compact.as_ref()),
            "1/10 matched (0 mismatched, 9 output failures; 1 transport errors, 1 skipped)"
        );
    }

    #[test]
    fn reduction_is_one_minus_the_ratio_and_keeps_its_sign() {
        assert_eq!(pct(100, 120), "-20.0%");
        assert_eq!(pct(100, 70), "+30.0%");
        assert_eq!(pct(100, 100), "+0.0%");
        assert_eq!(pct(0, 5), "n/a");
    }
}
