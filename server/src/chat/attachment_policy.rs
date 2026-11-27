const MIB: usize = 1024 * 1024;

pub(crate) const MAX_UPLOAD_FILE_BYTES: usize = 25 * MIB;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaClass {
    Image,
    Pdf,
    TextLike,
    Other,
}

impl MediaClass {
    pub(crate) fn limit(self) -> usize {
        match self {
            Self::Image | Self::Pdf => 25 * MIB,
            Self::TextLike => MIB,
            Self::Other => 10 * MIB,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Pdf => "PDF",
            Self::TextLike => "text",
            Self::Other => "other",
        }
    }
}

pub fn classify(bytes: &[u8], declared: &str, filename: &str) -> (MediaClass, String) {
    let declared = declared
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if let Some(sniffed) = sniff(bytes) {
        let class = match sniffed {
            "application/pdf" => MediaClass::Pdf,
            image if image.starts_with("image/") => MediaClass::Image,
            _ => MediaClass::Other,
        };
        let mime = if sniffed == "application/zip" && is_zip_container(&declared) {
            declared
        } else {
            sniffed.to_string()
        };
        return (class, mime);
    }
    let claims_image_or_pdf = declared.starts_with("image/") || declared == "application/pdf";
    // Text is text whatever it is called. A `.py`, `.log` or `.toml` file, or a `.ts` a browser
    // labels `video/mp2t`, would otherwise be stored as an unknown file, which no model provider
    // reads, so the router would refuse every turn that carries it.
    if !claims_image_or_pdf && is_text(bytes) {
        let mime = text_mime(&declared, filename).unwrap_or_else(|| "text/plain".to_string());
        return (MediaClass::TextLike, mime);
    }
    let harmless = !declared.is_empty() && !claims_image_or_pdf && !is_text_like(&declared);
    let mime = if harmless {
        declared
    } else {
        "application/octet-stream".to_string()
    };
    (MediaClass::Other, mime)
}

fn sniff(bytes: &[u8]) -> Option<&'static str> {
    let at = |range: std::ops::Range<usize>| bytes.get(range);
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xFF\xD8\xFF") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    if bytes.starts_with(b"PK\x03\x04") {
        return Some("application/zip");
    }
    if at(0..4) == Some(b"RIFF") {
        match at(8..12) {
            Some(b"WEBP") => return Some("image/webp"),
            Some(b"WAVE") => return Some("audio/wav"),
            _ => {}
        }
    }
    if at(4..8) == Some(b"ftyp") {
        match at(8..12) {
            Some(b"heic" | b"heix" | b"hevc" | b"heim" | b"heis") => return Some("image/heic"),
            Some(b"mif1" | b"msf1") => return Some("image/heif"),
            Some(b"isom" | b"iso2" | b"mp41" | b"mp42" | b"avc1" | b"M4V " | b"mmp4") => {
                return Some("video/mp4");
            }
            Some(b"M4A ") => return Some("audio/mp4"),
            _ => {}
        }
    }
    let mp3_frame = matches!(bytes, [0xFF, second, ..] if second & 0xE0 == 0xE0);
    if bytes.starts_with(b"ID3") || mp3_frame {
        return Some("audio/mpeg");
    }
    None
}

fn is_zip_container(declared: &str) -> bool {
    declared.starts_with("application/vnd.openxmlformats-officedocument.")
        || declared.starts_with("application/vnd.oasis.opendocument.")
        || declared == "application/epub+zip"
}

fn is_text_like(mime: &str) -> bool {
    mime.starts_with("text/")
        || matches!(
            mime,
            "application/json" | "application/xml" | "application/x-yaml" | "application/yaml"
        )
}

/// UTF-8 with no NUL byte: the test git and grep use to tell text from binary.
fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

/// The precise type of a text file whose declared type or extension names one.
fn text_mime(declared: &str, filename: &str) -> Option<String> {
    if is_text_like(declared) {
        return Some(declared.to_string());
    }
    let extension = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "txt" => "text/plain",
        "md" => "text/markdown",
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "html" | "htm" => "text/html",
        _ => return None,
    };
    Some(mime.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
    const JPEG: &[u8] = b"\xFF\xD8\xFF\xE0\0\x10JFIF";
    const GIF: &[u8] = b"GIF89a\x01\0";
    const WEBP: &[u8] = b"RIFF\x24\0\0\0WEBPVP8 ";
    const PDF: &[u8] = b"%PDF-1.7\n%\xE2\xE3";
    const HEIC: &[u8] = b"\0\0\0\x18ftypheic\0\0\0\0";
    const HEIF: &[u8] = b"\0\0\0\x18ftypmif1\0\0\0\0";
    const WAV: &[u8] = b"RIFF\x24\0\0\0WAVEfmt ";
    const MP3_ID3: &[u8] = b"ID3\x04\0\0\0\0\0\0";
    const MP3_SYNC: &[u8] = b"\xFF\xFB\x90\x64\0\0";
    const MP4: &[u8] = b"\0\0\0\x20ftypisom\0\0\x02\0";
    const ZIP: &[u8] = b"PK\x03\x04\x14\0\0\0";
    const DOCX: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

    fn class(bytes: &[u8], declared: &str, filename: &str) -> (MediaClass, String) {
        classify(bytes, declared, filename)
    }

    #[test]
    fn real_image_and_pdf_bytes_set_the_type_whatever_the_name_says() {
        let cases: [(&[u8], &str, &str, MediaClass, &str); 7] = [
            (
                PNG,
                "application/pdf",
                "photo.pdf",
                MediaClass::Image,
                "image/png",
            ),
            (JPEG, "image/png", "a.png", MediaClass::Image, "image/jpeg"),
            (GIF, "", "a", MediaClass::Image, "image/gif"),
            (
                WEBP,
                "application/octet-stream",
                "a.bin",
                MediaClass::Image,
                "image/webp",
            ),
            (
                HEIC,
                "image/heic",
                "a.heic",
                MediaClass::Image,
                "image/heic",
            ),
            (HEIF, "", "a.heif", MediaClass::Image, "image/heif"),
            (
                PDF,
                "text/plain",
                "notes.txt",
                MediaClass::Pdf,
                "application/pdf",
            ),
        ];
        for (bytes, declared, name, expected_class, expected_mime) in cases {
            assert_eq!(
                class(bytes, declared, name),
                (expected_class, expected_mime.to_string()),
                "{name} declared {declared}"
            );
        }
    }

    #[test]
    fn audio_and_video_bytes_are_other_files_with_their_real_type() {
        let cases: [(&[u8], &str); 4] = [
            (WAV, "audio/wav"),
            (MP3_ID3, "audio/mpeg"),
            (MP3_SYNC, "audio/mpeg"),
            (MP4, "video/mp4"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(
                class(bytes, "application/octet-stream", "f"),
                (MediaClass::Other, expected.to_string())
            );
        }
    }

    #[test]
    fn a_name_or_header_claiming_image_or_pdf_without_the_bytes_is_not_trusted() {
        for (declared, name) in [
            ("image/png", "x.png"),
            ("application/pdf", "x.pdf"),
            ("image/jpeg", "x.html"),
        ] {
            assert_eq!(
                class(b"<html><script>alert(1)</script></html>", declared, name),
                (MediaClass::Other, "application/octet-stream".to_string()),
                "{declared}"
            );
        }
    }

    #[test]
    fn utf8_text_with_a_text_type_or_extension_is_text() {
        let cases = [
            ("text/plain", "a.txt", "text/plain"),
            ("text/csv; charset=utf-8", "a.csv", "text/csv"),
            ("application/json", "a", "application/json"),
            ("application/octet-stream", "notes.md", "text/markdown"),
            ("", "data.yml", "application/yaml"),
            ("", "page.HTML", "text/html"),
            ("application/x-yaml", "a", "application/x-yaml"),
        ];
        for (declared, name, expected) in cases {
            assert_eq!(
                class("héllo, world\n".as_bytes(), declared, name),
                (MediaClass::TextLike, expected.to_string()),
                "{name} declared {declared}"
            );
        }
    }

    #[test]
    fn utf8_text_of_any_name_or_declared_type_is_plain_text() {
        let cases = [
            ("application/octet-stream", "service.py"),
            ("", "server.log"),
            ("application/octet-stream", "settings.toml"),
            ("video/mp2t", "service.ts"),
            ("application/x-sh", "run.sh"),
            ("application/sql", "schema.sql"),
            ("", "Makefile"),
        ];
        for (declared, name) in cases {
            assert_eq!(
                class("OWNER = \"Zoë\"\nPORT = 7110\n".as_bytes(), declared, name),
                (MediaClass::TextLike, "text/plain".to_string()),
                "{name} declared {declared}"
            );
        }
        assert_eq!(
            class(b"a\tb\n", "text/tab-separated-values", "hosts.tsv"),
            (
                MediaClass::TextLike,
                "text/tab-separated-values".to_string()
            )
        );
    }

    #[test]
    fn text_that_is_not_utf8_or_holds_a_nul_is_another_file() {
        assert_eq!(
            class(b"caf\xE9\n", "text/plain", "latin1.txt"),
            (MediaClass::Other, "application/octet-stream".to_string())
        );
        assert_eq!(
            class(b"header\0body", "application/octet-stream", "data.bin"),
            (MediaClass::Other, "application/octet-stream".to_string())
        );
    }

    #[test]
    fn unknown_bytes_keep_a_harmless_declared_type() {
        assert_eq!(
            class(ZIP, DOCX, "report.docx"),
            (MediaClass::Other, DOCX.to_string())
        );
        assert_eq!(
            class(ZIP, "application/octet-stream", "a.zip"),
            (MediaClass::Other, "application/zip".to_string())
        );
        assert_eq!(
            class(b"\x00\x01\x02", "application/x-thing", "a.thing"),
            (MediaClass::Other, "application/x-thing".to_string())
        );
        assert_eq!(
            class(b"\x00\x01\x02", "", "a"),
            (MediaClass::Other, "application/octet-stream".to_string())
        );
    }

    #[test]
    fn an_empty_file_is_classified_without_panicking() {
        assert_eq!(
            class(b"", "text/plain", "empty.txt"),
            (MediaClass::TextLike, "text/plain".to_string())
        );
        assert_eq!(
            class(b"", "image/png", "empty.png"),
            (MediaClass::Other, "application/octet-stream".to_string())
        );
        assert_eq!(
            class(b"\x89P", "", "short"),
            (MediaClass::Other, "application/octet-stream".to_string())
        );
    }

    #[test]
    fn limits_follow_the_approved_sizes() {
        assert_eq!(MediaClass::Image.limit(), 25 * 1024 * 1024);
        assert_eq!(MediaClass::Pdf.limit(), 25 * 1024 * 1024);
        assert_eq!(MediaClass::TextLike.limit(), 1024 * 1024);
        assert_eq!(MediaClass::Other.limit(), 10 * 1024 * 1024);
        assert_eq!(MAX_UPLOAD_FILE_BYTES, 25 * 1024 * 1024);
    }
}
