use crate::content_type::TypeMask;
use crate::error::CompressError;

/// How hard the compressors try. Every per-type tuning knob is derived from this in one place,
/// so a level change is a single edit rather than seven.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    #[default]
    Conservative,
    Balanced,
    Aggressive,
}

impl Level {
    pub const fn as_label(self) -> &'static str {
        match self {
            Level::Conservative => "conservative",
            Level::Balanced => "balanced",
            Level::Aggressive => "aggressive",
        }
    }

    pub fn parse(label: &str) -> Result<Self, CompressError> {
        match label.trim().to_ascii_lowercase().as_str() {
            "conservative" => Ok(Level::Conservative),
            "balanced" => Ok(Level::Balanced),
            "aggressive" => Ok(Level::Aggressive),
            other => Err(CompressError::UnknownLevel(other.to_string())),
        }
    }

    /// Lines kept at the head and tail of a log before eliding the middle.
    pub(crate) const fn log_head_tail(self) -> (usize, usize) {
        match self {
            Level::Conservative => (10, 10),
            Level::Balanced => (5, 5),
            Level::Aggressive => (3, 3),
        }
    }

    /// Elements kept at the head and tail of a homogeneous JSON array.
    pub(crate) const fn json_array_head_tail(self) -> (usize, usize) {
        match self {
            Level::Conservative => (3, 1),
            Level::Balanced => (2, 1),
            Level::Aggressive => (1, 1),
        }
    }

    /// Diff context lines kept either side of a change.
    pub(crate) const fn diff_context(self) -> usize {
        match self {
            Level::Conservative => 3,
            Level::Balanced => 2,
            Level::Aggressive => 1,
        }
    }

    /// Search hits kept at the head and tail.
    pub(crate) const fn search_head_tail(self) -> (usize, usize) {
        match self {
            Level::Conservative => (10, 3),
            Level::Balanced => (5, 2),
            Level::Aggressive => (3, 1),
        }
    }

    /// Longest JSON string value left intact, in characters.
    pub(crate) const fn max_string_chars(self) -> usize {
        match self {
            Level::Conservative => 2000,
            Level::Balanced => 800,
            Level::Aggressive => 300,
        }
    }
}

/// What the caller wants compressed, and how hard.
///
/// Constructed by the caller — this crate never reads the environment.
/// `Copy` because it is plain configuration — every field is a scalar or a shared reference.
/// Callers embed it in their own config structs, several of which are themselves `Copy`.
#[derive(Debug, Clone, Copy)]
pub struct Policy<'r> {
    pub enabled: bool,

    /// Payloads below this size are skipped: compressing them costs more than it saves.
    pub min_bytes: usize,

    /// Above this, head/tail truncate without detection or parsing. Checked *before* detection,
    /// because the Json rule requires a full parse and a huge malformed blob must not pay for one.
    pub max_input_bytes: usize,

    pub types: TypeMask,
    pub level: Level,

    /// Compute statistics but return the input unchanged. Lets a seam be measured in production
    /// before it is allowed to mutate anything.
    pub dry_run: bool,

    /// Recovery handle minted *by the caller*, interpolated into elision markers. Minting and
    /// persisting are RNG and I/O, both of which this crate forbids itself.
    pub recovery_ref: Option<&'r str>,
}

impl Default for Policy<'_> {
    fn default() -> Self {
        Self {
            enabled: false,
            min_bytes: 2048,
            max_input_bytes: 1 << 20,
            types: TypeMask::DEFAULT,
            level: Level::Conservative,
            dry_run: false,
            recovery_ref: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_off_so_adding_the_crate_changes_nothing() {
        let p = Policy::default();
        assert!(!p.enabled);
        assert!(!p.dry_run);
        assert_eq!(p.level, Level::Conservative);
    }

    #[test]
    fn level_parse_round_trips() {
        for level in [Level::Conservative, Level::Balanced, Level::Aggressive] {
            assert_eq!(Level::parse(level.as_label()).unwrap(), level);
        }
    }

    #[test]
    fn level_parse_is_case_insensitive_and_trims() {
        assert_eq!(Level::parse("  Balanced ").unwrap(), Level::Balanced);
    }

    #[test]
    fn level_parse_rejects_unknown() {
        assert_eq!(
            Level::parse("turbo").unwrap_err(),
            CompressError::UnknownLevel("turbo".into())
        );
    }

    #[test]
    fn aggressiveness_is_monotonic() {
        let levels = [Level::Conservative, Level::Balanced, Level::Aggressive];
        for pair in levels.windows(2) {
            let (looser, tighter) = (pair[0], pair[1]);
            assert!(looser.log_head_tail().0 >= tighter.log_head_tail().0);
            assert!(looser.diff_context() >= tighter.diff_context());
            assert!(looser.max_string_chars() >= tighter.max_string_chars());
            assert!(looser.search_head_tail().0 >= tighter.search_head_tail().0);
        }
    }
}
