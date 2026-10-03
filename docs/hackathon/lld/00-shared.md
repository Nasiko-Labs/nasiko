# Shared fixtures and grammar

Used by the task designs in this folder. Not a task.

Shared fixture used by the library tests. Put it in `tool-compact/src/lib.rs` under
`#[cfg(test)]`, or in `tool-compact/tests/common/mod.rs` if the tests live outside the crate.

```rust
fn calendar() -> ToolDef {
    ToolDef {
        name: "create_calendar_event".into(),
        description: Some("Create an event in the user's calendar.".into()),
        parameters: Some(json!({
            "type": "object",
            "properties": {
                "title": {"type": "string", "description": "Event title"},
                "start": {"type": "string", "format": "date-time", "description": "Start time, ISO 8601"},
                "duration_min": {"type": "integer", "description": "Duration in minutes"},
                "attendees": {"type": "array", "items": {"type": "string"}, "description": "Attendee emails"},
                "visibility": {"type": "string", "enum": ["public", "private"]}
            },
            "required": ["title", "start"]
        })),
    }
}

fn design_review() -> &'static str {
    r#"<<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>"#
}
```

Grammar, one place in `grammar.rs`:

```text
signature   := name "(" [param ("," param)*] ")" " - " description "\n"
instruction := "To call a tool, emit: <<call name {json args}>>"
param       := ident ":" type | ident "?:" type
type        := "str" | "int" | "num" | "bool" | "datetime"
             | alt ("|" alt)+
             | "[" type "]"
             | "{" param ("," param)* "}"
call        := "<<call " name " " json-object ">>"
```

The closer is the two characters `>>` only when the scanner is outside a JSON string.
Descriptions are copied verbatim in this slice.
