//! A stress set of realistic tool schemas, none of them from the public eval sample.
//!
//! Each tool is compacted on its own and held to three checks: it encodes, the schema comes back
//! exactly from `decode_tools`, and a rendered call decodes back to itself. Tools that use a
//! schema feature the compact form does not carry must be refused, with the reason naming the
//! feature. Run with `--nocapture` to see the report.

use nasiko_tool_compact::{
    CompactError, ToolCall, ToolDef, decode_calls, decode_tools, encode_tools, render_calls,
};
use serde_json::{Value, json};

enum Expect {
    Compacts,
    /// Bypassed; the reason must mention this.
    Bypassed(&'static str),
}

struct Case {
    tool: ToolDef,
    /// A call the schema accepts.
    good: Value,
    /// Calls the schema rejects.
    bad: Vec<Value>,
    expect: Expect,
}

fn tool(name: &str, description: Option<&str>, parameters: Option<Value>) -> ToolDef {
    ToolDef {
        name: name.into(),
        description: description.map(str::to_string),
        parameters,
    }
}

fn compacts(tool: ToolDef, good: Value, bad: Vec<Value>) -> Case {
    Case {
        tool,
        good,
        bad,
        expect: Expect::Compacts,
    }
}

fn bypassed(tool: ToolDef, because: &'static str) -> Case {
    Case {
        tool,
        good: Value::Null,
        bad: Vec::new(),
        expect: Expect::Bypassed(because),
    }
}

fn cases() -> Vec<Case> {
    vec![
        // Nested object, string lengths, integer ranges, nullable format, defaults.
        compacts(
            tool(
                "search_flights",
                Some("Search for flights between two airports."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "origin": {"type": "string", "minLength": 3, "maxLength": 3, "description": "IATA code, e.g. 'BLR'"},
                        "destination": {"type": "string", "minLength": 3, "maxLength": 3},
                        "depart_date": {"type": "string", "format": "date"},
                        "return_date": {"type": ["string", "null"], "format": "date", "description": "null for one-way"},
                        "passengers": {
                            "type": "object",
                            "properties": {
                                "adults": {"type": "integer", "minimum": 1, "maximum": 9},
                                "children": {"type": "integer", "minimum": 0, "default": 0},
                                "infants": {"type": "integer", "minimum": 0}
                            },
                            "required": ["adults"],
                            "additionalProperties": false
                        },
                        "cabin": {"type": "string", "enum": ["economy", "premium_economy", "business", "first"], "default": "economy"},
                        "nonstop": {"type": "boolean", "default": false},
                        "max_price": {"type": "number", "minimum": 0}
                    },
                    "required": ["origin", "destination", "depart_date", "passengers"]
                })),
            ),
            json!({"origin": "BLR", "destination": "DEL", "depart_date": "2026-11-02", "return_date": null,
                   "passengers": {"adults": 2, "children": 1}, "cabin": "business", "nonstop": true, "max_price": 450.5}),
            vec![
                json!({"origin": "BLRX", "destination": "DEL", "depart_date": "d", "passengers": {"adults": 1}}),
                json!({"origin": "BLR", "destination": "DEL", "depart_date": "d", "passengers": {"adults": 0}}),
                json!({"origin": "BLR", "destination": "DEL", "depart_date": "d", "passengers": {"adults": 1}, "cabin": "luxury"}),
                json!({"origin": "BLR", "destination": "DEL", "depart_date": "d", "passengers": {"adults": 1, "pets": 1}}),
            ],
        ),
        // Array of objects with a minimum item count.
        compacts(
            tool(
                "create_invoice",
                Some("Create a draft invoice for a customer."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "customer_id": {"type": "string"},
                        "line_items": {
                            "type": "array",
                            "minItems": 1,
                            "description": "At least one line",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "sku": {"type": "string"},
                                    "quantity": {"type": "integer", "minimum": 1},
                                    "unit_price": {"type": "number", "minimum": 0},
                                    "discount_pct": {"type": "number", "minimum": 0, "maximum": 100}
                                },
                                "required": ["sku", "quantity", "unit_price"]
                            }
                        },
                        "currency": {"type": "string", "enum": ["USD", "EUR", "INR"]},
                        "notes": {"type": "string", "maxLength": 500},
                        "due_in_days": {"type": "integer", "default": 30}
                    },
                    "required": ["customer_id", "line_items", "currency"]
                })),
            ),
            json!({"customer_id": "c_9", "currency": "INR", "due_in_days": 14,
                   "line_items": [{"sku": "A-1", "quantity": 2, "unit_price": 99.5}, {"sku": "B-2", "quantity": 1, "unit_price": 10, "discount_pct": 12.5}]}),
            vec![
                json!({"customer_id": "c", "currency": "INR", "line_items": []}),
                json!({"customer_id": "c", "currency": "INR", "line_items": [{"sku": "A", "quantity": 0, "unit_price": 1}]}),
                json!({"customer_id": "c", "currency": "INR", "line_items": [{"sku": "A", "quantity": 1, "unit_price": 1, "discount_pct": 150}]}),
                json!({"customer_id": "c", "currency": "GBP", "line_items": [{"sku": "A", "quantity": 1, "unit_price": 1}]}),
            ],
        ),
        // Number, boolean and integer enums.
        compacts(
            tool(
                "set_thermostat",
                Some("Set the target temperature."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "target_c": {"type": "number", "enum": [16.5, 18, 20.5, 22]},
                        "mode": {"type": "string", "enum": ["heat", "cool", "auto", "off"]},
                        "hold": {"type": "boolean", "enum": [true], "description": "Must be confirmed"},
                        "fan_speed": {"type": "integer", "enum": [1, 2, 3]}
                    },
                    "required": ["target_c", "mode"]
                })),
            ),
            json!({"target_c": 18, "mode": "heat", "hold": true, "fan_speed": 2}),
            vec![
                json!({"target_c": 19, "mode": "heat"}),
                json!({"target_c": 18, "mode": "heat", "hold": false}),
                json!({"target_c": 18, "mode": "heat", "fan_speed": 4}),
                json!({"target_c": "18", "mode": "heat"}),
            ],
        ),
        // Nullable scalars, a nullable enum, a nullable array, a closed object.
        compacts(
            tool(
                "update_user_profile",
                Some("Update fields on a user's profile; null clears a field."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "user_id": {"type": "string"},
                        "nickname": {"type": ["string", "null"], "maxLength": 30},
                        "age": {"type": ["integer", "null"], "minimum": 13, "maximum": 120},
                        "newsletter": {"type": ["boolean", "null"]},
                        "plan": {"type": ["string", "null"], "enum": ["free", "pro", null]},
                        "tags": {"type": ["array", "null"], "items": {"type": "string"}}
                    },
                    "required": ["user_id"],
                    "additionalProperties": false
                })),
            ),
            json!({"user_id": "u_42", "nickname": null, "age": 34, "newsletter": null, "plan": null, "tags": null}),
            vec![
                json!({"user_id": "u", "age": 12}),
                json!({"user_id": "u", "plan": "enterprise"}),
                json!({"user_id": null}),
                json!({"user_id": "u", "is_admin": true}),
            ],
        ),
        // No parameters at all.
        compacts(
            tool(
                "get_server_time",
                Some("Return the server's current time (UTC)."),
                None,
            ),
            json!({}),
            vec![json!({"tz": "UTC"})],
        ),
        // No description, empty properties.
        compacts(
            tool(
                "ping",
                None,
                Some(json!({"type": "object", "properties": {}})),
            ),
            json!({}),
            vec![json!({"x": 1})],
        ),
        // Apostrophes, double quotes, newlines and non-ASCII text in descriptions and values.
        compacts(
            tool(
                "translate_text",
                Some(
                    "Translate text.\nKeeps the author's \"voice\" — 日本語, العربية and emoji 🙂 are fine.",
                ),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "text": {"type": "string", "description": "May contain 'single' and \"double\" quotes"},
                        "target_lang": {"type": "string", "description": "BCP-47 tag, e.g. \"pt-BR\" or 'zh-Hant'"},
                        "glossary": {
                            "type": "array",
                            "description": "Terms that mustn't change:\n\tone per entry",
                            "items": {
                                "type": "object",
                                "properties": {"term": {"type": "string"}, "translation": {"type": "string"}},
                                "required": ["term", "translation"]
                            }
                        },
                        "formality": {"type": "string", "enum": ["formal", "informal", "don't care"]}
                    },
                    "required": ["text", "target_lang"]
                })),
            ),
            json!({"text": "She said \"it's fine\" >> then left\n— 終わり", "target_lang": "pt-BR",
                   "glossary": [{"term": "naïve", "translation": "ingênuo"}], "formality": "don't care"}),
            vec![json!({"text": "x", "target_lang": "fr", "formality": "dont care"})],
        ),
        // Defaults of every scalar kind, including null.
        compacts(
            tool(
                "run_sql_query",
                Some("Run a read-only SQL query."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "minLength": 1},
                        "dialect": {"type": "string", "enum": ["postgres", "mysql", "sqlite"], "default": "postgres"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 10000, "default": 100},
                        "timeout_s": {"type": "number", "default": 2.5},
                        "dry_run": {"type": "boolean", "default": true},
                        "role": {"type": ["string", "null"], "default": null},
                        "schema": {"type": "string", "default": "it's public"}
                    },
                    "required": ["query"]
                })),
            ),
            json!({"query": "select 1", "limit": 10, "timeout_s": 0.5, "dry_run": false, "role": null}),
            vec![
                json!({"query": ""}),
                json!({"query": "q", "limit": 0}),
                json!({"query": "q", "limit": 10001}),
            ],
        ),
        // Formats, enum values that are not identifiers, array and item bounds.
        compacts(
            tool(
                "upload_file",
                Some("Register a file that is already at a URL."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "url": {"type": "string", "format": "uri"},
                        "content_type": {"type": "string", "enum": ["image/png", "image/jpeg", "application/pdf"]},
                        "size_bytes": {"type": "integer", "minimum": 0, "maximum": 52428800},
                        "tags": {"type": "array", "maxItems": 3, "items": {"type": "string", "maxLength": 8}},
                        "owner_email": {"type": "string", "format": "email"},
                        "public": {"type": "boolean"}
                    },
                    "required": ["url", "content_type"]
                })),
            ),
            json!({"url": "https://example.com/a.png", "content_type": "image/png", "size_bytes": 1024, "tags": ["a", "b"], "public": false}),
            vec![
                json!({"url": "u", "content_type": "text/html"}),
                json!({"url": "u", "content_type": "image/png", "tags": ["a", "b", "c", "d"]}),
                json!({"url": "u", "content_type": "image/png", "tags": ["much-too-long"]}),
                json!({"url": "u", "content_type": "image/png", "size_bytes": -1}),
            ],
        ),
        // Four levels of nesting, a free-form object, a fractional bound.
        compacts(
            tool(
                "schedule_job",
                Some("Schedule a recurring background job."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string"},
                        "schedule": {
                            "type": "object",
                            "properties": {
                                "cron": {"type": "string", "description": "5-field cron"},
                                "timezone": {"type": "string", "default": "UTC"},
                                "retry": {
                                    "type": "object",
                                    "properties": {
                                        "max_attempts": {"type": "integer", "minimum": 0, "maximum": 10, "default": 3},
                                        "backoff": {
                                            "type": "object",
                                            "properties": {
                                                "kind": {"type": "string", "enum": ["fixed", "exponential"]},
                                                "seconds": {"type": "number", "minimum": 0.5}
                                            },
                                            "required": ["kind"]
                                        }
                                    }
                                }
                            },
                            "required": ["cron"]
                        },
                        "payload": {"type": "object", "description": "Passed to the job as-is"},
                        "enabled": {"type": "boolean"}
                    },
                    "required": ["name", "schedule"]
                })),
            ),
            json!({"name": "nightly", "enabled": true, "payload": {"any": ["thing", 1, null]},
                   "schedule": {"cron": "0 2 * * *", "retry": {"max_attempts": 5, "backoff": {"kind": "exponential", "seconds": 1.5}}}}),
            vec![
                json!({"name": "n", "schedule": {"cron": "c", "retry": {"backoff": {"kind": "fixed", "seconds": 0.1}}}}),
                json!({"name": "n", "schedule": {"cron": "c"}, "payload": "text"}),
                json!({"name": "n", "schedule": {"retry": {}}}),
            ],
        ),
        // Negative and fractional number ranges.
        compacts(
            tool(
                "geo_lookup",
                Some("Find places near a coordinate."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "lat": {"type": "number", "minimum": -90, "maximum": 90},
                        "lon": {"type": "number", "minimum": -180, "maximum": 180},
                        "radius_km": {"type": "number", "minimum": 0.1, "maximum": 500, "default": 1.5},
                        "units": {"type": "string", "enum": ["km", "mi"]}
                    },
                    "required": ["lat", "lon"]
                })),
            ),
            json!({"lat": 12.9716, "lon": 77.5946, "radius_km": 2}),
            vec![
                json!({"lat": 91, "lon": 0}),
                json!({"lat": 0, "lon": -180.5}),
                json!({"lat": 0, "lon": 0, "radius_km": 0}),
            ],
        ),
        // A dotted tool name, keys that are not identifiers, enum values that look like syntax.
        compacts(
            tool(
                "crm.contacts-search_v2",
                Some("Search contacts - by name, e-mail or size."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "first name": {"type": "string"},
                        "e-mail": {"type": "string", "format": "email"},
                        "größe": {"type": "integer", "enum": [-1, 0, 1]},
                        "is_vip?": {"type": "boolean"},
                        "kind": {"type": "string", "enum": ["null", "str", "42", "two words", "a|b", ""]},
                        "only": {"type": "string", "enum": ["exactly-this"]}
                    },
                    "required": ["first name"]
                })),
            ),
            json!({"first name": "Asha", "e-mail": "a@example.com", "größe": -1, "is_vip?": true, "kind": "a|b", "only": "exactly-this"}),
            vec![
                json!({"first name": "A", "kind": "null "}),
                json!({"first name": "A", "größe": 2}),
                json!({"first_name": "A"}),
            ],
        ),
        // A free-form object beside a nullable bounded integer.
        compacts(
            tool(
                "store_metadata",
                Some("Attach arbitrary metadata to a key."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "key": {"type": "string", "minLength": 1, "maxLength": 64},
                        "metadata": {"type": "object", "description": "Arbitrary JSON"},
                        "ttl_s": {"type": ["integer", "null"], "minimum": 0, "default": null}
                    },
                    "required": ["key", "metadata"],
                    "additionalProperties": false
                })),
            ),
            json!({"key": "k", "metadata": {"a": {"b": [1, 2, {"c": null}]}}, "ttl_s": 60}),
            vec![
                json!({"key": "k", "metadata": [1]}),
                json!({"key": "k", "metadata": {}, "ttl_s": -1}),
                json!({"key": "", "metadata": {}}),
            ],
        ),
        // Nullable integer and number enums.
        compacts(
            tool(
                "rate_item",
                Some("Rate an item from 1 to 5."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "item_id": {"type": "string"},
                        "rating": {"type": ["integer", "null"], "enum": [1, 2, 3, 4, 5, null]},
                        "weight": {"type": ["number", "null"], "enum": [0.5, 1, null]},
                        "anonymous": {"type": "boolean", "default": false},
                        "comment": {"type": "string", "maxLength": 280}
                    },
                    "required": ["item_id", "rating"]
                })),
            ),
            json!({"item_id": "i1", "rating": null, "weight": 0.5, "comment": "ok"}),
            vec![
                json!({"item_id": "i1", "rating": 6}),
                json!({"item_id": "i1", "rating": 5, "weight": 0.75}),
                json!({"item_id": "i1"}),
            ],
        ),
        // ── must be bypassed ──────────────────────────────────────────────────────────────
        bypassed(
            tool(
                "send_notification",
                Some("Notify a user by email or by phone."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "target": {"oneOf": [
                            {"type": "object", "properties": {"email": {"type": "string"}}, "required": ["email"]},
                            {"type": "object", "properties": {"phone": {"type": "string"}}, "required": ["phone"]}
                        ]},
                        "message": {"type": "string"}
                    },
                    "required": ["target", "message"]
                })),
            ),
            "oneOf",
        ),
        bypassed(
            tool(
                "find_contact",
                Some("Look a contact up (schema as Pydantic emits it)."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "query": {"anyOf": [{"type": "string"}, {"type": "null"}], "description": "Name or email"}
                    }
                })),
            ),
            "anyOf",
        ),
        bypassed(
            tool(
                "validate_code",
                Some("Check a promo code."),
                Some(json!({
                    "type": "object",
                    "properties": {"code": {"type": "string", "pattern": "^[A-Z]{4}-[0-9]{4}$"}},
                    "required": ["code"]
                })),
            ),
            "pattern",
        ),
        bypassed(
            tool(
                "merge_records",
                Some("Merge string fields into a record."),
                Some(json!({
                    "type": "object",
                    "properties": {
                        "record_id": {"type": "string"},
                        "fields": {"type": "object", "additionalProperties": {"type": "string"}}
                    },
                    "required": ["record_id", "fields"]
                })),
            ),
            "additionalProperties",
        ),
        bypassed(
            tool(
                "create_ticket",
                Some("Open a support ticket."),
                Some(json!({
                    "type": "object",
                    "$defs": {"Priority": {"type": "string", "enum": ["low", "high"]}},
                    "properties": {"title": {"type": "string"}},
                    "required": ["title"]
                })),
            ),
            "$defs",
        ),
        bypassed(
            tool(
                "set_volume",
                Some("Set playback volume."),
                Some(json!({
                    "type": "object",
                    "properties": {"level": {"type": "number", "exclusiveMinimum": 0, "maximum": 11}},
                    "required": ["level"]
                })),
            ),
            "exclusiveMinimum",
        ),
    ]
}

#[test]
fn stress_set_compacts_round_trips_or_is_refused_for_a_stated_reason() {
    let cases = cases();
    let mut compacted = 0;
    let mut bypassed = Vec::new();
    let (mut native_bytes, mut compact_bytes) = (0, 0);

    println!(
        "\n{:<24} {:<10} {:>7} {:>8}  detail",
        "tool", "result", "native", "compact"
    );
    for case in &cases {
        let name = &case.tool.name;
        let tools = std::slice::from_ref(&case.tool);
        let native = serde_json::to_string(&case.tool).unwrap().len();

        let compact = match (encode_tools(tools), &case.expect) {
            (Ok(compact), Expect::Compacts) => compact,
            (Err(CompactError::Unsupported { reason, .. }), Expect::Bypassed(because)) => {
                assert!(reason.contains(because), "{name}: reason was `{reason}`");
                println!(
                    "{name:<24} {:<10} {native:>7} {:>8}  {reason}",
                    "bypassed", "-"
                );
                bypassed.push((name.clone(), reason));
                continue;
            }
            (other, _) => panic!("{name}: unexpected outcome {other:?}"),
        };

        // 1. The schema comes back exactly.
        assert_eq!(
            decode_tools(&compact).unwrap(),
            tools,
            "{name}: schema changed"
        );

        // 2. A valid call, rendered, decodes back to itself.
        let call = ToolCall {
            name: name.clone(),
            arguments: case.good.to_string(),
        };
        let rendered = render_calls(std::slice::from_ref(&call)).unwrap();
        assert_eq!(
            decode_calls(&rendered, tools).unwrap(),
            vec![call],
            "{name}: call changed"
        );

        // 3. Calls the schema rejects are errors.
        for bad in &case.bad {
            let output = format!("<<call {name} {bad}>>");
            let error = decode_calls(&output, tools).expect_err(&format!("{name}: accepted {bad}"));
            assert_eq!(error.as_label(), "invalid_arguments", "{name}: {bad}");
        }

        compacted += 1;
        native_bytes += native;
        compact_bytes += compact.definitions.len();
        println!(
            "{name:<24} {:<10} {native:>7} {:>8}  schema exact, call round-trips, {} bad calls rejected",
            "compacted",
            compact.definitions.len(),
            case.bad.len()
        );
    }

    println!(
        "\n{compacted} of {} compacted, {} bypassed; compacted tools: {native_bytes} native bytes -> {compact_bytes} compact bytes",
        cases.len(),
        bypassed.len()
    );
    assert_eq!((compacted, bypassed.len()), (14, 6));
}

#[test]
fn one_unsupported_tool_bypasses_the_whole_request_and_names_itself() {
    let all: Vec<ToolDef> = cases().into_iter().map(|case| case.tool).collect();
    match encode_tools(&all) {
        Err(CompactError::Unsupported { tool, .. }) => assert_eq!(tool, "send_notification"),
        other => panic!("expected the first unsupported tool to be named, got {other:?}"),
    }
}

#[test]
fn the_supported_tools_compact_together_and_come_back_in_order() {
    let supported: Vec<ToolDef> = cases()
        .into_iter()
        .filter(|case| matches!(case.expect, Expect::Compacts))
        .map(|case| case.tool)
        .collect();
    let compact = encode_tools(&supported).unwrap();
    assert_eq!(compact.definitions.lines().count(), supported.len());
    assert_eq!(decode_tools(&compact).unwrap(), supported);
}
