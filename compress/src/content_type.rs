use crate::error::CompressError;

/// The shapes [`crate::detect`] can recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentType {
    Json,
    Log,
    Diff,
    SearchResults,
    Code,
    Markup,
    Prose,
    /// A test runner's report (unittest, Django's `runtests.py`, pytest).
    TestOutput,
}

impl ContentType {
    pub const COUNT: usize = 8;

    pub const ALL: [ContentType; Self::COUNT] = [
        ContentType::Json,
        ContentType::Log,
        ContentType::Diff,
        ContentType::SearchResults,
        ContentType::Code,
        ContentType::Markup,
        ContentType::Prose,
        ContentType::TestOutput,
    ];

    pub const fn as_label(self) -> &'static str {
        match self {
            ContentType::Json => "json",
            ContentType::Log => "log",
            ContentType::Diff => "diff",
            ContentType::SearchResults => "search",
            ContentType::Code => "code",
            ContentType::Markup => "markup",
            ContentType::Prose => "prose",
            ContentType::TestOutput => "test",
        }
    }

    pub fn parse(label: &str) -> Result<Self, CompressError> {
        match label.trim().to_ascii_lowercase().as_str() {
            "json" => Ok(ContentType::Json),
            "log" => Ok(ContentType::Log),
            "diff" => Ok(ContentType::Diff),
            "search" | "search_results" | "searchresults" => Ok(ContentType::SearchResults),
            "code" => Ok(ContentType::Code),
            "markup" | "html" => Ok(ContentType::Markup),
            "prose" | "text" => Ok(ContentType::Prose),
            "test" | "tests" | "test_output" => Ok(ContentType::TestOutput),
            other => Err(CompressError::UnknownContentType(other.to_string())),
        }
    }

    /// Stable index into a per-type counter array.
    pub const fn index(self) -> usize {
        self as usize
    }

    const fn bit(self) -> u8 {
        1u8 << (self as u8)
    }
}

/// Which content types a [`Policy`](crate::Policy) will compress.
///
/// A `u8` bitset rather than an `EnumSet` — an eight-element set does not justify a dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TypeMask(u8);

impl TypeMask {
    pub const NONE: Self = TypeMask(0);

    /// Every type, including the ones whose compressors are not written yet.
    pub const ALL: Self = TypeMask(0b1111_1111);

    /// `json | log | diff | test`.
    ///
    /// `test` is on because its compressor only drops what carries no failure information —
    /// passing-test lines, clean progress dots, repeated chatter — and keeps every failure block,
    /// traceback and summary verbatim.
    ///
    /// `search` is deliberately absent: its detector fires on ripgrep output, which for a coding
    /// agent is frequently the answer itself. Enabling it by default reproduces the tool-side
    /// pathology this whole project exists to avoid.
    pub const DEFAULT: Self = TypeMask(
        ContentType::Json.bit()
            | ContentType::Log.bit()
            | ContentType::Diff.bit()
            | ContentType::TestOutput.bit(),
    );

    pub const fn contains(self, kind: ContentType) -> bool {
        self.0 & kind.bit() != 0
    }

    pub const fn with(self, kind: ContentType) -> Self {
        TypeMask(self.0 | kind.bit())
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Parse a comma-separated list, e.g. `"json,log,diff"`. An empty list is [`Self::NONE`].
    pub fn from_labels(labels: &str) -> Result<Self, CompressError> {
        let mut mask = TypeMask::NONE;
        for label in labels.split(',') {
            let label = label.trim();
            if label.is_empty() {
                continue;
            }
            mask = mask.with(ContentType::parse(label)?);
        }
        Ok(mask)
    }

    pub fn labels(self) -> Vec<&'static str> {
        ContentType::ALL
            .iter()
            .filter(|k| self.contains(**k))
            .map(|k| k.as_label())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mask_is_json_log_diff_test_and_excludes_search() {
        assert!(TypeMask::DEFAULT.contains(ContentType::Json));
        assert!(TypeMask::DEFAULT.contains(ContentType::Log));
        assert!(TypeMask::DEFAULT.contains(ContentType::Diff));
        assert!(TypeMask::DEFAULT.contains(ContentType::TestOutput));
        assert!(!TypeMask::DEFAULT.contains(ContentType::Code));
        assert!(!TypeMask::DEFAULT.contains(ContentType::SearchResults));
        assert!(!TypeMask::DEFAULT.contains(ContentType::Prose));
    }

    #[test]
    fn all_mask_contains_every_variant() {
        for kind in ContentType::ALL {
            assert!(TypeMask::ALL.contains(kind), "{kind:?} missing from ALL");
        }
    }

    #[test]
    fn indices_are_distinct_and_within_count() {
        let mut seen = [false; ContentType::COUNT];
        for kind in ContentType::ALL {
            let i = kind.index();
            assert!(i < ContentType::COUNT);
            assert!(!seen[i], "duplicate index for {kind:?}");
            seen[i] = true;
        }
    }

    #[test]
    fn from_labels_round_trips_and_ignores_blanks() {
        let mask = TypeMask::from_labels(" json , ,log ").unwrap();
        assert_eq!(mask.labels(), vec!["json", "log"]);
    }

    #[test]
    fn from_labels_rejects_unknown_type() {
        let err = TypeMask::from_labels("json,nope").unwrap_err();
        assert_eq!(err, CompressError::UnknownContentType("nope".into()));
    }

    #[test]
    fn from_labels_empty_is_none() {
        assert_eq!(TypeMask::from_labels("  ").unwrap(), TypeMask::NONE);
    }
}
