use std::borrow::Cow;

pub(crate) fn normalize_text(s: &str) -> Cow<'_, str> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        Cow::Borrowed("")
    } else {
        Cow::Owned(trimmed.to_ascii_lowercase())
    }
}

pub(crate) fn contains_any(text: &str, needles: &[&str]) -> bool {
    let lower = normalize_text(text);
    needles.iter().any(|needle| lower.contains(*needle))
}

pub(crate) fn count_any(text: &str, needles: &[&str]) -> usize {
    let lower = normalize_text(text);
    needles.iter().filter(|needle| lower.contains(**needle)).count()
}

pub(crate) fn token_count(text: &str) -> usize {
    text.split_whitespace().count()
}
