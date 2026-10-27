//! Lossless table form for a JSON list of same-shaped records.
//!
//! An API list of 100 search results repeats every key 100 times; that repetition, not the data,
//! is most of its size. Here the keys are written once and each record becomes one row of values:
//!
//! ```text
//! [⋯ table: 100 rows ⋯]
//! columns: path:str, repository.full_name:str, repository.stars:int, score:num
//! src/a.py,golang/go,80901,11.84
//! ```
//!
//! Every record survives — [`expand_tables`] turns a table back into exactly the original list —
//! so this is the shape used when nothing may be dropped (no recovery handle). Nested objects
//! whose keys are the same in every record are flattened into dotted columns.
//!
//! Cell encoding, by column type: `int`, `num` and `bool` cells are JSON literals; a `str` cell is
//! written bare unless it is empty or could be misread (contains `,` `"` `\` or a line break,
//! starts with `[` `{` or a space, or ends with a space), in which case it is a JSON string;
//! `json` cells (nested lists, nulls, mixed types) are compact JSON. Splitting a row is therefore
//! a JSON-aware scan for top-level commas.

use serde_json::{Map, Value};

use crate::marker;

/// Fewer rows than this and the header costs about what the table saves.
const MIN_ROWS: usize = 4;
const COLUMNS_PREFIX: &str = "columns: ";

/// The table form of `items`, or `None` when they are not a list of same-shaped records.
pub(crate) fn tabulate(items: &[Value]) -> Option<String> {
    if items.len() < MIN_ROWS {
        return None;
    }
    let rows: Vec<Vec<(String, &Value)>> = items.iter().map(flatten).collect::<Option<_>>()?;
    let columns: Vec<&str> = rows[0].iter().map(|(path, _)| path.as_str()).collect();
    let same_shape = rows.iter().all(|row| {
        row.len() == columns.len() && row.iter().zip(&columns).all(|((p, _), c)| p == c)
    });
    if columns.len() < 2 || !same_shape {
        return None;
    }

    let types: Vec<ColumnType> = (0..columns.len())
        .map(|i| ColumnType::of(rows.iter().map(|row| row[i].1)))
        .collect();

    let mut out = marker::table_header(rows.len());
    out.push('\n');
    out.push_str(COLUMNS_PREFIX);
    let header: Vec<String> = columns
        .iter()
        .zip(&types)
        .map(|(c, t)| format!("{c}:{}", t.label()))
        .collect();
    out.push_str(&header.join(", "));
    for row in &rows {
        out.push('\n');
        let cells: Vec<String> = row
            .iter()
            .zip(&types)
            .map(|((_, v), t)| t.encode(v))
            .collect();
        out.push_str(&cells.join(","));
    }
    Some(out)
}

/// `text` with every table string inside it expanded back into its list of records, or `None`
/// when `text` is not JSON. A document with no tables comes back as itself.
pub fn expand_tables(text: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(text).ok()?;
    expand_value(value)
}

fn expand_value(value: Value) -> Option<Value> {
    Some(match value {
        Value::String(s) if s.starts_with(&marker::table_header_prefix()) => untabulate(&s)?,
        Value::Array(items) => {
            Value::Array(items.into_iter().map(expand_value).collect::<Option<_>>()?)
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| Some((k, expand_value(v)?)))
                .collect::<Option<Map<_, _>>>()?,
        ),
        other => other,
    })
}

fn untabulate(table: &str) -> Option<Value> {
    let mut lines = table.split('\n');
    lines.next()?; // header marker
    let columns: Vec<(String, ColumnType)> = lines
        .next()?
        .strip_prefix(COLUMNS_PREFIX)?
        .split(", ")
        .map(|c| {
            let (name, ty) = c.rsplit_once(':')?;
            Some((name.to_string(), ColumnType::parse(ty)?))
        })
        .collect::<Option<_>>()?;

    let records = lines
        .map(|line| {
            let cells = split_row(line)?;
            if cells.len() != columns.len() {
                return None;
            }
            let mut record = Map::new();
            for ((name, ty), cell) in columns.iter().zip(cells) {
                insert_path(&mut record, name, ty.decode(cell)?);
            }
            Some(Value::Object(record))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(Value::Array(records))
}

/// A record as `(dotted path, leaf)` pairs, flattening nested objects. `None` when the item is
/// not an object or a key could not be told apart from a path (it contains `.`, `,`, `:` or a
/// line break, or is empty).
fn flatten(item: &Value) -> Option<Vec<(String, &Value)>> {
    let Value::Object(map) = item else {
        return None;
    };
    let mut out = Vec::new();
    flatten_into(map, "", &mut out)?;
    Some(out)
}

fn flatten_into<'a>(
    map: &'a Map<String, Value>,
    prefix: &str,
    out: &mut Vec<(String, &'a Value)>,
) -> Option<()> {
    for (key, value) in map {
        if key.is_empty() || key.contains(['.', ',', ':', '\n']) || key.trim() != key {
            return None;
        }
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            Value::Object(inner) if !inner.is_empty() => flatten_into(inner, &path, out)?,
            leaf => out.push((path, leaf)),
        }
    }
    Some(())
}

fn insert_path(record: &mut Map<String, Value>, path: &str, value: Value) {
    match path.split_once('.') {
        None => {
            record.insert(path.to_string(), value);
        }
        Some((head, rest)) => {
            let child = record
                .entry(head.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if let Value::Object(inner) = child {
                insert_path(inner, rest, value);
            }
        }
    }
}

/// Split a row on commas outside JSON strings, arrays and objects.
fn split_row(line: &str) -> Option<Vec<&str>> {
    let bytes = line.as_bytes();
    let (mut cells, mut start, mut depth) = (Vec::new(), 0usize, 0i32);
    let (mut in_string, mut escaped) = (false, false);
    for (i, &b) in bytes.iter().enumerate() {
        if in_string {
            match b {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                cells.push(line.get(start..i)?);
                start = i + 1;
            }
            _ => {}
        }
    }
    cells.push(line.get(start..)?);
    Some(cells)
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ColumnType {
    Str,
    Int,
    Num,
    Bool,
    Json,
}

impl ColumnType {
    fn of<'a>(values: impl Iterator<Item = &'a Value>) -> Self {
        let mut ty: Option<ColumnType> = None;
        for v in values {
            let this = match v {
                Value::String(_) => ColumnType::Str,
                Value::Bool(_) => ColumnType::Bool,
                Value::Number(n) if n.is_i64() || n.is_u64() => ColumnType::Int,
                Value::Number(_) => ColumnType::Num,
                _ => return ColumnType::Json,
            };
            ty = Some(match (ty, this) {
                (None, t) => t,
                (Some(a), b) if a == b => a,
                (Some(ColumnType::Int | ColumnType::Num), ColumnType::Int | ColumnType::Num) => {
                    ColumnType::Num
                }
                _ => return ColumnType::Json,
            });
        }
        ty.unwrap_or(ColumnType::Json)
    }

    fn label(self) -> &'static str {
        match self {
            ColumnType::Str => "str",
            ColumnType::Int => "int",
            ColumnType::Num => "num",
            ColumnType::Bool => "bool",
            ColumnType::Json => "json",
        }
    }

    fn parse(label: &str) -> Option<Self> {
        Some(match label {
            "str" => ColumnType::Str,
            "int" => ColumnType::Int,
            "num" => ColumnType::Num,
            "bool" => ColumnType::Bool,
            "json" => ColumnType::Json,
            _ => return None,
        })
    }

    fn encode(self, v: &Value) -> String {
        match (self, v) {
            (ColumnType::Str, Value::String(s)) if is_bare_safe(s) => s.clone(),
            _ => v.to_string(),
        }
    }

    fn decode(self, cell: &str) -> Option<Value> {
        match self {
            ColumnType::Str if !cell.starts_with('"') => Some(Value::String(cell.to_string())),
            _ => serde_json::from_str(cell).ok(),
        }
    }
}

/// A string that can be written without quotes and read back unchanged.
fn is_bare_safe(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(['[', '{', '"', ' '])
        && !s.ends_with(' ')
        && !s.contains([',', '"', '\\', '\n', '\r'])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn search_results(n: usize) -> Vec<Value> {
        (0..n)
            .map(|i| {
                json!({
                    "path": format!("src/mod_{i}/handler.py"),
                    "repository": {"full_name": format!("org/repo{i}"), "stars": i * 7},
                    "score": 1.5 + i as f64,
                    "fork": i % 2 == 0,
                    "topics": ["auth", "jwt"],
                })
            })
            .collect()
    }

    fn round_trip(items: Vec<Value>) {
        let table = tabulate(&items).expect("should tabulate");
        let doc = json!({ "items": table });
        let back = expand_tables(&doc.to_string()).unwrap();
        assert_eq!(back, json!({ "items": items }), "table:\n{table}");
    }

    #[test]
    fn a_list_of_records_round_trips_exactly() {
        round_trip(search_results(20));
    }

    #[test]
    fn awkward_strings_round_trip() {
        let items: Vec<Value> = [
            "plain",
            "",
            "has, comma",
            "has \"quotes\"",
            "line\nbreak",
            " leading space",
            "trailing ",
            "[bracket",
            "{brace",
            "back\\slash",
            "12345",
            "true",
        ]
        .iter()
        .enumerate()
        .map(|(i, s)| json!({"id": i, "text": s}))
        .collect();
        round_trip(items);
    }

    #[test]
    fn mixed_and_null_values_round_trip() {
        let items = vec![
            json!({"id": 1, "v": 1, "note": null}),
            json!({"id": 2, "v": 2.5, "note": "x"}),
            json!({"id": 3, "v": "three", "note": [1, {"a": 2}]}),
            json!({"id": 4, "v": true, "note": []}),
        ];
        round_trip(items);
    }

    #[test]
    fn writes_keys_once_and_flattens_nested_objects() {
        // Column order follows serde_json's map order, which is insertion order when a crate in
        // the build enables `preserve_order` (the server binaries do) and sorted otherwise — so
        // compare columns as a set and leave exact contents to the round-trip tests.
        let table = tabulate(&search_results(5)).unwrap();
        let mut lines = table.lines();
        assert_eq!(lines.next(), Some("[⋯ table: 5 rows ⋯]"));
        let mut columns: Vec<&str> = lines
            .next()
            .and_then(|l| l.strip_prefix(COLUMNS_PREFIX))
            .unwrap()
            .split(", ")
            .collect();
        columns.sort_unstable();
        assert_eq!(
            columns,
            [
                "fork:bool",
                "path:str",
                "repository.full_name:str",
                "repository.stars:int",
                "score:num",
                "topics:json",
            ]
        );
        assert_eq!(lines.count(), 5, "one row per record");
        assert_eq!(
            table.matches("full_name").count(),
            1,
            "each key written once"
        );
    }

    #[test]
    fn declines_lists_that_are_not_same_shaped_records() {
        assert!(tabulate(&[json!(1), json!(2), json!(3), json!(4)]).is_none());
        let ragged = vec![
            json!({"a": 1, "b": 2}),
            json!({"a": 1, "b": 2}),
            json!({"a": 1, "c": 2}),
            json!({"a": 1, "b": 2}),
        ];
        assert!(tabulate(&ragged).is_none());
        assert!(tabulate(&search_results(3)).is_none(), "too few rows");
        let dotted: Vec<Value> = (0..5).map(|i| json!({"a.b": i, "c": i})).collect();
        assert!(
            tabulate(&dotted).is_none(),
            "a dotted key would read as a path"
        );
    }

    #[test]
    fn expand_leaves_a_document_without_tables_unchanged() {
        let doc = json!({"a": [1, 2, {"b": "c"}], "s": "plain"});
        assert_eq!(expand_tables(&doc.to_string()).unwrap(), doc);
    }
}
