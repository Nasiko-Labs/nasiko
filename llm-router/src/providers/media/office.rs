//! Word and Excel documents as the text they hold.
//!
//! No provider reads `.docx` or `.xlsx` files, so the router sends them on as text, the way it
//! already sends text documents: a Word document's paragraphs and tables, and an Excel workbook's
//! sheets row by row. Pictures, charts and formatting are not carried.
//!
//! Both are zip packages of XML. A package is read within [`UNPACKED_MAX_BYTES`] in all, so a small
//! archive that unpacks to gigabytes is refused rather than read.

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::{Cursor, Read};

use quick_xml::XmlVersion;
use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::reader::Reader;
use zip::ZipArchive;
use zip::result::ZipError;

use crate::ir::UnsupportedPart;

const WORD: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
const EXCEL: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";

/// The largest Word or Excel file read, as sent.
pub(crate) const PACKAGE_MAX_BYTES: u64 = 20 * 1024 * 1024;

/// The most XML read out of one package, all parts together.
const UNPACKED_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Empty cells kept between two values in a row; a wider gap is written as one.
const MAX_EMPTY_CELLS: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Office {
    Word,
    Excel,
}

impl Office {
    pub(crate) fn of(media_type: &str) -> Option<Self> {
        match media_type {
            WORD => Some(Self::Word),
            EXCEL => Some(Self::Excel),
            _ => None,
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Self::Word => "a Word document",
            Self::Excel => "an Excel workbook",
        }
    }

    /// The text `bytes` hold, as the model reads it.
    pub(crate) fn text(self, bytes: &[u8]) -> Result<String, UnsupportedPart> {
        let unreadable = || UnsupportedPart::new(format!("{} that cannot be read", self.noun()));
        let mut package = Package::open(bytes).ok_or_else(unreadable)?;
        let text = match self {
            Self::Word => word_text(&mut package),
            Self::Excel => excel_text(&mut package),
        };
        text.map_err(|e| match e {
            Unreadable::TooLarge => UnsupportedPart::new(format!(
                "{} over {} MiB once unpacked",
                self.noun(),
                UNPACKED_MAX_BYTES / (1024 * 1024)
            )),
            Unreadable::Malformed => unreadable(),
        })
    }
}

enum Unreadable {
    Malformed,
    TooLarge,
}

impl From<quick_xml::Error> for Unreadable {
    fn from(_: quick_xml::Error) -> Self {
        Self::Malformed
    }
}

struct Package<'a> {
    archive: ZipArchive<Cursor<&'a [u8]>>,
    unpacked_left: u64,
}

impl<'a> Package<'a> {
    fn open(bytes: &'a [u8]) -> Option<Self> {
        let archive = ZipArchive::new(Cursor::new(bytes)).ok()?;
        Some(Self {
            archive,
            unpacked_left: UNPACKED_MAX_BYTES,
        })
    }

    /// One XML part, or `None` when the package has no part by that name.
    fn part(&mut self, name: &str) -> Result<Option<String>, Unreadable> {
        let file = match self.archive.by_name(name) {
            Ok(file) => file,
            Err(ZipError::FileNotFound) => return Ok(None),
            Err(_) => return Err(Unreadable::Malformed),
        };
        let mut xml = String::new();
        file.take(self.unpacked_left + 1)
            .read_to_string(&mut xml)
            .map_err(|_| Unreadable::Malformed)?;
        let read = xml.len() as u64;
        if read > self.unpacked_left {
            return Err(Unreadable::TooLarge);
        }
        self.unpacked_left -= read;
        Ok(Some(xml))
    }

    fn required(&mut self, name: &str) -> Result<String, Unreadable> {
        self.part(name)?.ok_or(Unreadable::Malformed)
    }
}

fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn attribute<'e>(element: &'e BytesStart<'_>, name: &str) -> Option<Cow<'e, str>> {
    element
        .attributes()
        .flatten()
        .find(|a| local(a.key.as_ref()) == name)
        .and_then(|a| a.normalized_value(XmlVersion::Implicit1_0).ok())
}

/// The character an entity reference (`&amp;`, `&#x41;`) stands for, added to `out`.
fn push_reference(out: &mut String, reference: &BytesRef<'_>) {
    if let Ok(Some(ch)) = reference.resolve_char_ref() {
        out.push(ch);
    } else if let Some(text) = resolve_predefined_entity(reference) {
        out.push_str(text);
    }
}

// ─── Word ────────────────────────────────────────────────────────────────────

/// A Word document's body: one line per paragraph, a table's cells separated by tabs.
fn word_text(package: &mut Package<'_>) -> Result<String, Unreadable> {
    let xml = package.required("word/document.xml")?;
    let mut reader = Reader::from_str(&xml);
    let mut out = String::new();
    let (mut in_run, mut in_text) = (false, false);
    loop {
        match reader.read_event()? {
            Event::Start(e) => match local(e.name().as_ref()) {
                "r" => in_run = true,
                "t" => in_text = true,
                _ => {}
            },
            Event::End(e) => match local(e.name().as_ref()) {
                "r" => in_run = false,
                "t" => in_text = false,
                "p" => out.push('\n'),
                "tc" => {
                    if out.ends_with('\n') {
                        out.pop();
                    }
                    out.push('\t');
                }
                "tr" => {
                    if out.ends_with('\t') {
                        out.pop();
                    }
                    out.push('\n');
                }
                _ => {}
            },
            // A run's own tab or break; tab stops in paragraph properties are not text.
            Event::Empty(e) if in_run => match local(e.name().as_ref()) {
                "tab" => out.push('\t'),
                "br" | "cr" => out.push('\n'),
                _ => {}
            },
            Event::Text(t) if in_text => out.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if in_text => push_reference(&mut out, &r),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(out.trim_end().to_string())
}

// ─── Excel ───────────────────────────────────────────────────────────────────

/// Every sheet of a workbook, in workbook order: its name, then one line per row with the cells
/// separated by tabs. A cell shows its value (a formula's last computed result), not its formula.
fn excel_text(package: &mut Package<'_>) -> Result<String, Unreadable> {
    let shared = match package.part("xl/sharedStrings.xml")? {
        Some(xml) => shared_strings(&xml)?,
        None => Vec::new(),
    };
    let mut out = String::new();
    for (name, path) in sheets(package)? {
        let xml = package.required(&path)?;
        out.push_str(&format!("Sheet: {name}\n"));
        sheet_rows(&xml, &shared, &mut out)?;
        out.push('\n');
    }
    Ok(out.trim_end().to_string())
}

/// The workbook's sheets, in order, as (name, part path).
fn sheets(package: &mut Package<'_>) -> Result<Vec<(String, String)>, Unreadable> {
    let rels = package.required("xl/_rels/workbook.xml.rels")?;
    let mut targets = HashMap::new();
    let mut reader = Reader::from_str(&rels);
    loop {
        match reader.read_event()? {
            Event::Start(e) | Event::Empty(e) if local(e.name().as_ref()) == "Relationship" => {
                if let (Some(id), Some(target)) = (attribute(&e, "Id"), attribute(&e, "Target")) {
                    let path = match target.strip_prefix('/') {
                        Some(absolute) => absolute.to_string(),
                        None => format!("xl/{target}"),
                    };
                    targets.insert(id.into_owned(), path);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }

    let workbook = package.required("xl/workbook.xml")?;
    let mut sheets = Vec::new();
    let mut reader = Reader::from_str(&workbook);
    loop {
        match reader.read_event()? {
            Event::Start(e) | Event::Empty(e) if local(e.name().as_ref()) == "sheet" => {
                let name = attribute(&e, "name").unwrap_or(Cow::Borrowed("Sheet"));
                // `r:id`: the relationship that names the sheet's part.
                if let Some(path) = attribute(&e, "id").and_then(|id| targets.get(id.as_ref())) {
                    sheets.push((name.into_owned(), path.clone()));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(sheets)
}

/// The shared string table: one entry per `<si>`, its runs joined, phonetic hints left out.
fn shared_strings(xml: &str) -> Result<Vec<String>, Unreadable> {
    let mut strings = Vec::new();
    let mut current = String::new();
    let (mut in_text, mut in_phonetic) = (false, false);
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event()? {
            Event::Start(e) => match local(e.name().as_ref()) {
                "t" => in_text = !in_phonetic,
                "rPh" => in_phonetic = true,
                _ => {}
            },
            Event::End(e) => match local(e.name().as_ref()) {
                "t" => in_text = false,
                "rPh" => in_phonetic = false,
                "si" => strings.push(std::mem::take(&mut current)),
                _ => {}
            },
            Event::Empty(e) if local(e.name().as_ref()) == "si" => strings.push(String::new()),
            Event::Text(t) if in_text => current.push_str(&t.xml10_content()),
            Event::GeneralRef(r) if in_text => push_reference(&mut current, &r),
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(strings)
}

/// One cell being read: where it sits, its type (`t`), and the text of its value.
#[derive(Default)]
struct Cell {
    column: usize,
    kind: String,
    value: String,
}

impl Cell {
    fn resolved(self, shared: &[String]) -> String {
        match self.kind.as_str() {
            "s" => self
                .value
                .trim()
                .parse::<usize>()
                .ok()
                .and_then(|i| shared.get(i).cloned())
                .unwrap_or_default(),
            "b" => match self.value.trim() {
                "1" => "TRUE".to_string(),
                "0" => "FALSE".to_string(),
                other => other.to_string(),
            },
            _ => self.value,
        }
    }
}

fn sheet_rows(xml: &str, shared: &[String], out: &mut String) -> Result<(), Unreadable> {
    let mut row: Vec<(usize, String)> = Vec::new();
    let mut cell: Option<Cell> = None;
    let mut in_value = false;
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event()? {
            Event::Start(e) => match local(e.name().as_ref()) {
                "c" => {
                    let next = row.last().map_or(0, |(c, _)| c + 1);
                    cell = Some(Cell {
                        column: attribute(&e, "r")
                            .and_then(|r| column_index(&r))
                            .unwrap_or(next),
                        kind: attribute(&e, "t").unwrap_or_default().into_owned(),
                        value: String::new(),
                    });
                }
                // `<v>` holds the value; `<t>` an inline string's text. `<f>` (the formula) is skipped.
                "v" | "t" => in_value = cell.is_some(),
                _ => {}
            },
            Event::End(e) => match local(e.name().as_ref()) {
                "v" | "t" => in_value = false,
                "c" => {
                    if let Some(done) = cell.take() {
                        let column = done.column;
                        let value = done.resolved(shared);
                        if !value.is_empty() {
                            row.push((column, value));
                        }
                    }
                }
                "row" => write_row(out, std::mem::take(&mut row)),
                _ => {}
            },
            Event::Text(t) if in_value => {
                if let Some(c) = cell.as_mut() {
                    c.value.push_str(&t.xml10_content());
                }
            }
            Event::GeneralRef(r) if in_value => {
                if let Some(c) = cell.as_mut() {
                    push_reference(&mut c.value, &r);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(())
}

/// A row's values separated by tabs, an empty cell kept as an empty field so columns line up.
fn write_row(out: &mut String, row: Vec<(usize, String)>) {
    if row.is_empty() {
        return;
    }
    let mut next = 0;
    for (i, (column, value)) in row.into_iter().enumerate() {
        if i > 0 {
            out.push('\t');
        }
        let gap = column.saturating_sub(next).min(MAX_EMPTY_CELLS);
        out.push_str(&"\t".repeat(gap));
        out.push_str(&value);
        next = column + 1;
    }
    out.push('\n');
}

/// The zero-based column of a cell reference: `A1` → 0, `AB12` → 27.
fn column_index(reference: &str) -> Option<usize> {
    let letters = reference
        .bytes()
        .take_while(u8::is_ascii_alphabetic)
        .collect::<Vec<_>>();
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let number = letters.iter().fold(0usize, |n, &b| {
        n * 26 + usize::from(b.to_ascii_uppercase() - b'A' + 1)
    });
    Some(number - 1)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn package(parts: &[(&str, &str)]) -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in parts {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    /// A Word package whose body is `body`.
    pub(in crate::providers::media) fn word(body: &str) -> Vec<u8> {
        package(&[(
            "word/document.xml",
            &format!(
                r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{body}</w:body></w:document>"#
            ),
        )])
    }

    fn workbook(sheets: &[(&str, &str)], shared: Option<&str>) -> Vec<u8> {
        let mut parts = Vec::new();
        let mut listed = String::new();
        let mut rels = String::new();
        for (i, (name, rows)) in sheets.iter().enumerate() {
            let n = i + 1;
            listed.push_str(&format!(
                r#"<sheet name="{name}" sheetId="{n}" r:id="rId{n}"/>"#
            ));
            rels.push_str(&format!(
                r#"<Relationship Id="rId{n}" Type="worksheet" Target="worksheets/sheet{n}.xml"/>"#
            ));
            parts.push((
                format!("xl/worksheets/sheet{n}.xml"),
                format!("<worksheet><sheetData>{rows}</sheetData></worksheet>"),
            ));
        }
        parts.push((
            "xl/workbook.xml".into(),
            format!(
                r#"<workbook xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>{listed}</sheets></workbook>"#
            ),
        ));
        parts.push((
            "xl/_rels/workbook.xml.rels".into(),
            format!("<Relationships>{rels}</Relationships>"),
        ));
        if let Some(sst) = shared {
            parts.push(("xl/sharedStrings.xml".into(), format!("<sst>{sst}</sst>")));
        }
        let borrowed: Vec<(&str, &str)> = parts
            .iter()
            .map(|(n, b)| (n.as_str(), b.as_str()))
            .collect();
        package(&borrowed)
    }

    #[test]
    fn a_word_document_reads_as_its_paragraphs() {
        let docx = word(
            r#"<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="720"/></w:tabs></w:pPr>
                 <w:r><w:t>Contract</w:t></w:r><w:r><w:t xml:space="preserve"> CN-58213</w:t></w:r></w:p>
               <w:p><w:r><w:t>Price</w:t><w:tab/><w:t>4 &amp; 5 &#x20AC;</w:t></w:r></w:p>
               <w:p><w:r><w:t>Line one</w:t><w:br/><w:t>line two</w:t></w:r></w:p>"#,
        );
        assert_eq!(
            Office::Word.text(&docx).unwrap(),
            "Contract CN-58213\nPrice\t4 & 5 €\nLine one\nline two"
        );
    }

    #[test]
    fn a_word_table_reads_as_tab_separated_rows() {
        let cell = |t: &str| format!("<w:tc><w:p><w:r><w:t>{t}</w:t></w:r></w:p></w:tc>");
        let docx = word(&format!(
            "<w:tbl><w:tr>{}{}</w:tr><w:tr>{}{}</w:tr></w:tbl><w:p><w:r><w:t>After</w:t></w:r></w:p>",
            cell("Region"),
            cell("Units"),
            cell("Norrland"),
            cell("4812")
        ));
        assert_eq!(
            Office::Word.text(&docx).unwrap(),
            "Region\tUnits\nNorrland\t4812\nAfter"
        );
    }

    #[test]
    fn an_excel_workbook_reads_sheet_by_sheet_with_cell_values() {
        let xlsx = workbook(
            &[
                (
                    "Sales",
                    r#"<row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row>
                       <row r="2"><c r="A2" t="inlineStr"><is><t>Norrland</t></is></c><c r="B2"><v>4812</v></c></row>
                       <row r="3"><c r="A3" t="s"><v>2</v></c><c r="C3"><f>B2*2</f><v>9624</v></c></row>"#,
                ),
                (
                    "Flags",
                    r#"<row r="1"><c r="A1" t="b"><v>1</v></c><c r="B1" t="str"><v>A &lt; B</v></c></row>"#,
                ),
            ],
            Some(
                r#"<si><t>Region</t></si><si><r><t>Un</t></r><r><t>its</t></r></si>
                   <si><t>Vestfold</t><rPh><t>VF</t></rPh></si>"#,
            ),
        );
        assert_eq!(
            Office::Excel.text(&xlsx).unwrap(),
            "Sheet: Sales\nRegion\tUnits\nNorrland\t4812\nVestfold\t\t9624\n\nSheet: Flags\nTRUE\tA < B"
        );
    }

    #[test]
    fn a_wide_gap_between_cells_is_capped() {
        let xlsx = workbook(
            &[(
                "S",
                r#"<row r="1"><c r="A1"><v>1</v></c><c r="ZZ1"><v>2</v></c></row>"#,
            )],
            None,
        );
        let text = Office::Excel.text(&xlsx).unwrap();
        assert_eq!(
            text,
            format!("Sheet: S\n1\t{}2", "\t".repeat(MAX_EMPTY_CELLS))
        );
    }

    #[test]
    fn a_file_that_is_not_a_package_is_named_as_unreadable() {
        let err = Office::Word.text(b"not a zip").unwrap_err();
        assert_eq!(err.into_message(), "a Word document that cannot be read");
        let err = Office::Excel
            .text(&package(&[("other.xml", "<x/>")]))
            .unwrap_err();
        assert_eq!(err.into_message(), "an Excel workbook that cannot be read");
    }

    #[test]
    fn a_package_that_unpacks_past_the_limit_is_refused() {
        let bytes = word("<w:p/>");
        let mut package = Package::open(&bytes).unwrap();
        package.unpacked_left = 4;
        assert!(matches!(
            package.part("word/document.xml"),
            Err(Unreadable::TooLarge)
        ));
    }

    #[test]
    fn column_letters_count_from_zero() {
        assert_eq!(column_index("A1"), Some(0));
        assert_eq!(column_index("Z9"), Some(25));
        assert_eq!(column_index("AB12"), Some(27));
        assert_eq!(column_index("12"), None);
    }

    #[test]
    fn only_word_and_excel_are_office_documents_here() {
        assert_eq!(Office::of(WORD), Some(Office::Word));
        assert_eq!(Office::of(EXCEL), Some(Office::Excel));
        assert_eq!(Office::of("application/msword"), None);
        assert_eq!(Office::of("application/zip"), None);
    }
}
