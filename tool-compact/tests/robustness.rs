//! Decoder robustness under mutation. This tests the DECODER only. It cannot show that a real
//! model follows the format: decoder robustness is verified offline; live adherence remains
//! unmeasured until a live run exists.
//!
//! Classes:
//! * `prose`: text around a valid call (required support) must decode to the identical calls.
//! * `syntax`: a deviation from the published grammar must never yield the original calls
//!   (no repair): the result is an error or no call.
//! * `random`: seeded character mutations; whatever comes out must validate against the schema.
//!
//! Run `cargo test -p nasiko-tool-compact --test robustness -- --nocapture` for the table.

use std::collections::BTreeMap;

use nasiko_tool_compact::{DecodeError, ToolCall, ToolDef, decode_calls, validate_call};
use serde_json::{Value, json};

fn tools() -> Vec<ToolDef> {
    vec![
        ToolDef {
            name: "send_email".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {
                    "to": {"type": "array", "items": {"type": "string"}},
                    "subject": {"type": "string"},
                    "body": {"type": "string"},
                    "prio": {"type": "string", "enum": ["low", "high"]}
                },
                "required": ["to", "subject", "body"]
            })),
        },
        ToolDef {
            name: "set_timer".into(),
            description: None,
            parameters: Some(json!({
                "type": "object",
                "properties": {"seconds": {"type": "integer"}, "label": {"type": "string"}},
                "required": ["seconds"]
            })),
        },
    ]
}

const BASES: [&str; 4] = [
    r#"<<call send_email {"body":"hello >> world","subject":"s","to":["a@b.c"]}>>"#,
    r#"<<call set_timer {"label":"tea","seconds":180}>>"#,
    r#"<<call send_email {"body":"é 😀 \" \\ }","prio":"high","subject":"s","to":["a@b.c","d@e.f"]}>>"#,
    r#"<<call set_timer {"seconds":5}>>"#,
];

type Outcome = Result<Vec<ToolCall>, DecodeError>;

#[derive(Default)]
struct Tally(BTreeMap<&'static str, usize>);

impl Tally {
    fn add(&mut self, key: &'static str) {
        *self.0.entry(key).or_default() += 1;
    }
}

fn classify(out: &Outcome, original: &Outcome) -> &'static str {
    match out {
        Err(_) => "rejected",
        Ok(c) if c.is_empty() => "no_call",
        Ok(_) if out == original => "same_as_original",
        Ok(_) => "different_but_valid",
    }
}

/// Every call that comes out must satisfy the original schema.
fn assert_all_valid(out: &Outcome, tools: &[ToolDef], what: &str) {
    if let Ok(calls) = out {
        for call in calls {
            let args: Value = serde_json::from_str(&call.arguments).expect("canonical json");
            validate_call(&call.name, &args, tools).unwrap_or_else(|e| panic!("{what}: {e}"));
        }
    }
}

#[test]
fn prose_around_a_valid_call_is_accepted() {
    let tools = tools();
    for base in BASES {
        let original = decode_calls(base, &tools);
        assert!(original.as_ref().is_ok_and(|c| c.len() == 1), "{base}");
        let variants = [
            format!("Sure! {base}"),
            format!("{base}\nDone."),
            format!("Here you go:\n\n{base}\n\nLet me know."),
            format!("```\n{base}\n```"),
            format!("Thinking <...> done.\n{base}"),
            format!("a << b >> c {base} d > e"),
        ];
        for v in &variants {
            assert_eq!(decode_calls(v, &tools), original, "{v}");
        }
    }
}

#[test]
fn grammar_deviations_are_never_repaired() {
    let tools = tools();
    let mut tally = Tally::default();
    for base in BASES {
        let original = decode_calls(base, &tools);
        let name_end = base.find(" {").unwrap_or(0);
        let json_start = name_end + 1;
        let deviations: Vec<(&str, String)> = vec![
            ("space_before_closer", base.replace("}>>", "} >>")),
            ("newline_before_closer", base.replace("}>>", "}\n>>")),
            ("newline_after_name", base.replacen(" {", "\n{", 1)),
            ("tab_after_name", base.replacen(" {", "\t{", 1)),
            ("double_space", base.replacen("<<call ", "<<call  ", 1)),
            ("no_space_after_name", base.replacen(" {", "{", 1)),
            ("uppercase_marker", base.replacen("<<call", "<<CALL", 1)),
            (
                "single_closer",
                base.trim_end_matches('>').to_string() + ">",
            ),
            ("no_closer", base.trim_end_matches('>').to_string()),
            ("missing_brace", base.replacen("}>>", ">>", 1)),
            ("extra_brace", base.replacen("}>>", "}}>>", 1)),
            ("trailing_comma", base.replacen("}>>", ",}>>", 1)),
            ("single_quotes", base.replace('"', "'")),
            (
                "json_in_array",
                format!(
                    "{}[{}]>>",
                    &base[..json_start],
                    &base[json_start..base.len() - 2]
                ),
            ),
            ("string_args", format!("{}\"x\">>", &base[..json_start])),
            ("colon_style", base.replacen("<<call ", "<<call:", 1)),
        ];
        for (name, text) in deviations {
            let out = decode_calls(&text, &tools);
            assert_ne!(out, original, "{name} was repaired: {text}");
            assert_all_valid(&out, &tools, name);
            tally.add(classify(&out, &original));
        }
    }
    println!("grammar deviations (decoder robustness): {:?}", tally.0);
}

/// Small deterministic generator so the run is reproducible without extra dependencies.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

#[test]
fn seeded_random_mutations_never_panic_and_never_emit_invalid_calls() {
    let tools = tools();
    let junk = [
        '}', '{', '"', '>', '<', ',', ':', '\\', ' ', '\n', 'x', '0', 'é', '😀',
    ];
    let mut rng = Lcg(0x5EED);
    let mut tally = Tally::default();
    for base in BASES {
        let original = decode_calls(base, &tools);
        let chars: Vec<char> = base.chars().collect();
        for _ in 0..4000 {
            let mut c = chars.clone();
            for _ in 0..(1 + rng.next() % 3) {
                let at = (rng.next() as usize) % c.len().max(1);
                match rng.next() % 4 {
                    0 if !c.is_empty() => {
                        c.remove(at);
                    }
                    1 => c.insert(at, junk[(rng.next() as usize) % junk.len()]),
                    2 if !c.is_empty() => c[at] = junk[(rng.next() as usize) % junk.len()],
                    _ if c.len() > 1 => {
                        let last = c.len() - 2;
                        let a = at.min(last);
                        c.swap(a, a + 1);
                    }
                    _ => {}
                }
            }
            let text: String = c.into_iter().collect();
            let out = decode_calls(&text, &tools);
            assert_all_valid(&out, &tools, &text);
            tally.add(classify(&out, &original));
        }
    }
    println!(
        "random mutations x16000 (decoder robustness): {:?}",
        tally.0
    );
    assert!(tally.0.values().sum::<usize>() == 16000);
}
