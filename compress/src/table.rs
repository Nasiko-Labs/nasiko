//! Lossless table form for a JSON list of records.
//!
//! An API list of 100 search results repeats every key 100 times; that repetition, not the data,
//! is most of its size. Here the keys are written once and each record becomes one row of values:
//!
//! ```text
//! [⋯ table: 100 rows ⋯]
//! columns: path:str, repository.full_name:str, repository.stars:int, score:num, url:str, error:str?
//! src/a.py,golang/go,80901,11.84,https://github.com/golang/go/blob/main/src/a.py,
//! ```
//!
//! Every cell holds its whole value, even where every row repeats it. Two header shortcuts were
//! tried and dropped because models misread them in lookups: stating a column that is the same in
//! every row once (`same:`), which models ignored and answered from other columns instead, and
//! keeping only the part after a shared prefix (`prefix:`), which models returned as the value
//! (`04T07:00:00Z` for `2024-01-04T07:00:00Z`).
//!
//! Records need not share every key. The columns are every key any record has, and a column some
//! records lack is marked `?` after its type; an empty cell there means "this record has no such
//! key", distinct from an empty string (`""`) and from `null`. Real lists are ragged like this — a
//! few error entries or optional fields among regular results — and one odd record must not cost
//! the whole list its table.
//!
//! Every record survives — [`expand_tables`] turns a table back into exactly the original list —
//! so this is the shape used when nothing may be dropped (no recovery handle). Nested objects are
//! flattened into dotted columns.
//!
//! Cell encoding, by column type: `int`, `num` and `bool` cells are JSON literals; a `str` cell is
//! written bare unless it is empty or could be misread (contains `,` `"` `\` or a line break,
//! starts with `[` `{` or a space, or ends with a space), in which case it is a JSON string;
//! `json` cells (nested lists, nulls, mixed types) are compact JSON. Splitting a row is therefore
//! a JSON-aware scan for top-level commas. No present value encodes to an empty cell, which is
//! what keeps a missing key unambiguous.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::marker;

/// Fewer rows than this and the header costs about what the table saves.
const MIN_ROWS: usize = 4;
const COLUMNS_PREFIX: &str = "columns: ";
/// Follows a column's type when some records lack that key (`error:str?`).
const OPTIONAL_MARK: &str = "?";

/// The table form of `items`, or `None` when they are not a list of records.
pub(crate) fn tabulate(items: &[Value]) -> Option<String> {
    if items.len() < MIN_ROWS {
        return None;
    }
    let flat: Vec<Vec<(String, &Value)>> = items.iter().map(flatten).collect::<Option<_>>()?;
    // Every key any record has, in first-seen order. Matching by name, not position, also makes
    // two records that list the same keys in a different order share their columns.
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut columns: Vec<&str> = Vec::new();
    for (path, _) in flat.iter().flatten() {
        index.entry(path.as_str()).or_insert_with(|| {
            columns.push(path.as_str());
            columns.len() - 1
        });
    }
    if columns.len() < 2 {
        return None;
    }
    let rows: Vec<Vec<Option<&Value>>> = flat
        .iter()
        .map(|record| {
            let mut cells = vec![None; columns.len()];
            for (path, value) in record {
                cells[index[path.as_str()]] = Some(*value);
            }
            cells
        })
        .collect();
    let optional: Vec<bool> = (0..columns.len())
        .map(|i| rows.iter().any(|row| row[i].is_none()))
        .collect();
    let types: Vec<ColumnType> = (0..columns.len())
        .map(|i| ColumnType::of(rows.iter().filter_map(|row| row[i])))
        .collect();

    let mut out = marker::table_header(rows.len());
    out.push('\n');
    out.push_str(COLUMNS_PREFIX);
    let header: Vec<String> = types
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mark = if optional[i] { OPTIONAL_MARK } else { "" };
            format!("{}:{}{mark}", columns[i], t.label())
        })
        .collect();
    out.push_str(&header.join(", "));
    for row in &rows {
        out.push('\n');
        let cells: Vec<String> = row
            .iter()
            .zip(&types)
            .map(|(cell, t)| match cell {
                // A missing key is an empty cell; no present value encodes to one.
                None => String::new(),
                Some(v) => t.encode(v),
            })
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
    let columns: Vec<(String, ColumnType, bool)> = lines
        .next()?
        .strip_prefix(COLUMNS_PREFIX)?
        .split(", ")
        .map(|c| {
            let (name, ty) = c.rsplit_once(':')?;
            let (ty, optional) = match ty.strip_suffix(OPTIONAL_MARK) {
                Some(ty) => (ty, true),
                None => (ty, false),
            };
            Some((name.to_string(), ColumnType::parse(ty)?, optional))
        })
        .collect::<Option<_>>()?;

    let records = lines
        .map(|line| {
            let cells = split_row(line)?;
            if cells.len() != columns.len() {
                return None;
            }
            let mut record = Map::new();
            for ((name, ty, optional), cell) in columns.iter().zip(cells) {
                if *optional && cell.is_empty() {
                    continue;
                }
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
        // `split_row` counts bracket depth to find cell boundaries, so a bare bracket would
        // shift every boundary after it.
        && !s.contains([',', '"', '\\', '\n', '\r', '[', ']', '{', '}'])
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
    fn every_row_holds_every_value_whole() {
        let items: Vec<Value> = (0..12)
            .map(|i| {
                json!({
                    "url": format!("https://github.com/org/repo{i}/blob/main/a.py"),
                    "ts": format!("2026-10-27T10:00:{i:02}Z"),
                    "service": "payment",
                    "code": 500,
                })
            })
            .collect();
        let table = tabulate(&items).unwrap();
        assert!(!table.contains("same:"), "{table}");
        assert!(!table.contains("prefix:"), "{table}");
        for whole in [
            "https://github.com/org/repo3/blob/main/a.py",
            "2026-10-27T10:00:03Z",
        ] {
            assert!(table.contains(whole), "cells hold whole values: {table}");
        }
        assert_eq!(
            table.matches("payment").count(),
            12,
            "a value every row shares stays in every row: {table}"
        );
        round_trip(items);
    }

    #[test]
    fn a_few_odd_records_keep_the_table_and_mark_their_columns_optional() {
        // The shape of real search results: regular hits plus a needle and an error entry that
        // carry keys no other record has.
        let mut items = search_results(10);
        items[3]["uuid"] = json!("9b2f-needle");
        items[7]["error"] = json!("upstream timeout");
        let table = tabulate(&items).expect("ragged records still tabulate");
        let columns = table
            .lines()
            .find_map(|l| l.strip_prefix(COLUMNS_PREFIX))
            .unwrap();
        assert!(columns.contains("uuid:str?"), "{table}");
        assert!(columns.contains("error:str?"), "{table}");
        assert!(columns.contains("path:str"), "{table}");
        assert!(
            !columns.contains("path:str?"),
            "a key every record has stays required"
        );
        assert_eq!(
            table.matches("uuid").count(),
            1,
            "the key is still written once"
        );
        round_trip(items);
    }

    /// The odd value sits in a middle column: a stray bracket in the last cell has no later
    /// boundary to shift, so it would round-trip even without quoting.
    #[test]
    fn strings_with_brackets_inside_round_trip() {
        for odd in ["a]0", "x}y", "a]", "a[b", "f{x", "[a"] {
            let items: Vec<Value> = (0..6)
                .map(|i| {
                    let s = if i == 2 {
                        odd.to_string()
                    } else {
                        format!("val{i}")
                    };
                    json!({"n": i, "s": s, "t": format!("row {i}")})
                })
                .collect();
            round_trip(items);
        }
    }

    #[test]
    fn a_missing_key_null_and_empty_string_stay_distinct() {
        let items = vec![
            json!({"id": 1, "note": "x"}),
            json!({"id": 2, "note": ""}),
            json!({"id": 3, "note": null}),
            json!({"id": 4}),
            json!({"id": 5, "note": "y"}),
        ];
        round_trip(items);
    }

    #[test]
    fn missing_nested_keys_and_a_missing_object_round_trip() {
        let items = vec![
            json!({"id": 1, "meta": {"author": "a", "tags": ["x"]}}),
            json!({"id": 2, "meta": {"author": "b"}}),
            json!({"id": 3}),
            json!({"id": 4, "meta": null}),
            json!({"id": 5, "meta": {}}),
        ];
        round_trip(items);
    }

    #[test]
    fn the_same_keys_in_another_order_share_columns() {
        let items: Vec<Value> = (0..6)
            .map(|i| {
                let mut m = Map::new();
                if i % 2 == 0 {
                    m.insert("a".into(), json!(i));
                    m.insert("b".into(), json!(format!("v{i}")));
                } else {
                    m.insert("b".into(), json!(format!("v{i}")));
                    m.insert("a".into(), json!(i));
                }
                Value::Object(m)
            })
            .collect();
        let table = tabulate(&items).unwrap();
        assert!(!table.contains('?'), "no key is missing anywhere: {table}");
        round_trip(items);
    }

    #[test]
    fn declines_lists_that_are_not_records() {
        assert!(tabulate(&[json!(1), json!(2), json!(3), json!(4)]).is_none());
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
