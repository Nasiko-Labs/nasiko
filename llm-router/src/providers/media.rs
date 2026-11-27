use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};

use crate::ir::{DataSource, UnsupportedPart};

mod office;

use office::Office;

pub(crate) const TEXT_DOCUMENT_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DocumentKind {
    Pdf,
    /// Sent to the model as text ([`decode_text`]): a text file as it is, a Word or Excel file as
    /// the text it holds.
    TextLike,
    Other,
}

pub(crate) fn document_kind(media_type: &str) -> DocumentKind {
    match media_type {
        "application/pdf" => DocumentKind::Pdf,
        m if is_text_like(m) || Office::of(m).is_some() => DocumentKind::TextLike,
        _ => DocumentKind::Other,
    }
}

pub(crate) fn is_text_like(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || matches!(
            media_type,
            "application/json" | "application/xml" | "application/x-yaml" | "application/yaml"
        )
}

pub(crate) fn decoded_len(b64: &str) -> u64 {
    b64.trim_end_matches('=').len() as u64 * 3 / 4
}

pub(crate) fn decode_text(source: &DataSource) -> Result<String, UnsupportedPart> {
    let Some((media_type, data)) = source.inline() else {
        return Err(UnsupportedPart::new("a text document given by URL"));
    };
    let office = Office::of(media_type);
    let (limit, too_large) = match office {
        Some(_) => (
            office::PACKAGE_MAX_BYTES,
            "a Word or Excel file over 20 MiB",
        ),
        None => (TEXT_DOCUMENT_MAX_BYTES, "a text document over 1 MiB"),
    };
    if decoded_len(data) > limit {
        return Err(UnsupportedPart::new(too_large));
    }
    let bytes = STANDARD
        .decode(data)
        .or_else(|_| STANDARD_NO_PAD.decode(data))
        .map_err(|_| UnsupportedPart::new("a text document that is not valid base64"))?;
    let text = match office {
        Some(office) => {
            let text = office.text(&bytes)?;
            if text.len() as u64 > TEXT_DOCUMENT_MAX_BYTES {
                return Err(UnsupportedPart::new(
                    "a Word or Excel file holding over 1 MiB of text",
                ));
            }
            text
        }
        None => String::from_utf8(bytes)
            .map_err(|_| UnsupportedPart::new("a text document that is not UTF-8"))?,
    };
    if text.is_empty() {
        return Err(UnsupportedPart::new("an empty text document"));
    }
    Ok(text)
}

pub(crate) fn text_document(
    source: &DataSource,
    filename: Option<&str>,
) -> Result<String, UnsupportedPart> {
    let name = filename
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or("document");
    Ok(format!("File: {name}\n\n{}", decode_text(source)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inline(media_type: &str, bytes: &[u8]) -> DataSource {
        DataSource::from_inline(media_type, &STANDARD.encode(bytes))
    }

    #[test]
    fn the_decoded_size_comes_from_the_base64_length_alone() {
        assert_eq!(decoded_len(""), 0);
        assert_eq!(decoded_len("QQ=="), 1);
        assert_eq!(decoded_len("QUI="), 2);
        assert_eq!(decoded_len("QUJD"), 3);
        assert_eq!(decoded_len("QUJDRA"), 4);
        assert_eq!(
            decoded_len(&STANDARD.encode(vec![0u8; 1_048_577])),
            1_048_577
        );
    }

    #[test]
    fn documents_are_sorted_into_pdf_text_and_everything_else() {
        assert_eq!(document_kind("application/pdf"), DocumentKind::Pdf);
        for text in [
            "text/plain",
            "text/csv",
            "text/markdown",
            "application/json",
            "application/x-yaml",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ] {
            assert_eq!(document_kind(text), DocumentKind::TextLike, "{text}");
        }
        for other in [
            "application/msword",
            "application/vnd.ms-excel",
            "application/zip",
            "application/octet-stream",
        ] {
            assert_eq!(document_kind(other), DocumentKind::Other, "{other}");
        }
    }

    #[test]
    fn a_text_document_decodes_to_its_utf8_text() {
        assert_eq!(
            decode_text(&inline("text/plain", "résumé – 東京".as_bytes())).unwrap(),
            "résumé – 東京"
        );
        let unpadded = DataSource::from_inline("text/plain", "aGk");
        assert_eq!(decode_text(&unpadded).unwrap(), "hi");
    }

    #[test]
    fn a_text_document_the_model_could_not_read_is_named() {
        let cases = [
            (inline("text/plain", &[0xff, 0xfe]), "not UTF-8"),
            (inline("text/plain", b""), "empty"),
            (inline("text/plain", &vec![b'a'; 1_048_577]), "over 1 MiB"),
            (
                DataSource::from_inline("text/plain", "!!!not base64"),
                "not valid base64",
            ),
            (
                DataSource::Url("https://example.com/a.txt".into()),
                "by URL",
            ),
        ];
        for (source, expected) in cases {
            let message = decode_text(&source).unwrap_err().into_message();
            assert!(message.contains(expected), "{message}");
        }
        assert!(decode_text(&inline("text/plain", &vec![b'a'; 1_048_576])).is_ok());
    }

    #[test]
    fn a_word_or_excel_file_decodes_to_the_text_it_holds() {
        let docx = office::tests::word(r#"<w:p><w:r><w:t>Signed in Bergen</w:t></w:r></w:p>"#);
        let source = inline(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            &docx,
        );
        assert_eq!(decode_text(&source).unwrap(), "Signed in Bergen");
        let named = text_document(&source, Some("contract.docx")).unwrap();
        assert_eq!(named, "File: contract.docx\n\nSigned in Bergen");

        let broken = inline(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            b"PK\x03\x04",
        );
        let message = decode_text(&broken).unwrap_err().into_message();
        assert_eq!(message, "an Excel workbook that cannot be read");
    }
}
