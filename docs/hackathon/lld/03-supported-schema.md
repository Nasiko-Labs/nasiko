# Task 3 — Supported schema subset

Tracked in [`../TODO.md`](../TODO.md).

Fixtures and the grammar live in [`00-shared.md`](00-shared.md).

`schema.rs` exposes `fn classify(tool: &ToolDef) -> Result<Shape, CompactError>`.
`Shape` is an owned tree: `Scalar(Str | Int | Num | Bool | DateTime)`, `Enum(Vec<String>)`,
`Array(Box<Shape>)`, `Object { fields: Vec<Field> }` where `Field { name, required, shape }`.
Walk `parameters`. Missing `parameters` is an empty object.

Reject, with `UnsupportedSchema` and the feature name, when the walker sees `$ref`,
`oneOf`, `anyOf`, `allOf`, `not`, a non-boolean `additionalProperties`, `prefixItems`,
an array `items` that is itself an array, a non-string `enum` entry, or a `format`
other than `date-time`. Do not drop the feature and continue.

`encode_tools` calls `classify` on every tool first. One rejection fails the whole batch
so the caller can bypass. Do not return a compact form for a mixed batch.

### Tests

```rust
#[test]
fn calendar_schema_is_supported() {
    let shape = classify(&calendar()).unwrap();
    let fields = shape.fields();
    assert!(fields.iter().any(|f| f.name == "title" && f.required && f.shape == Shape::Scalar(Scalar::Str)));
    assert!(fields.iter().any(|f| f.name == "duration_min" && !f.required));
    assert!(fields.iter().any(|f| f.name == "visibility" && matches!(f.shape, Shape::Enum(ref v) if v == ["public", "private"])));
    assert!(fields.iter().any(|f| f.name == "attendees" && matches!(f.shape, Shape::Array(_))));
}

#[test]
fn ref_schema_is_unsupported_and_not_simplified() {
    let tool = ToolDef {
        name: "lookup".into(),
        description: None,
        parameters: Some(json!({"type": "object", "properties": {"id": {"$ref": "#/$defs/Id"}}})),
    };
    let err = classify(&tool).unwrap_err();
    assert!(matches!(err, CompactError::UnsupportedSchema { feature, .. } if feature == "$ref"));
}
```
