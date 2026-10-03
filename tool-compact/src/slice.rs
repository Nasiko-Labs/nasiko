//! UTF-8 safe slicing helpers that satisfy clippy::string_slice and clippy::unwrap_used.

#[inline]
pub(crate) fn safe_slice(s: &str, start: usize, end: usize) -> &str {
    s.get(start..end).unwrap_or_default()
}

#[inline]
pub(crate) fn safe_slice_from(s: &str, start: usize) -> &str {
    s.get(start..).unwrap_or_default()
}

#[inline]
pub(crate) fn safe_slice_to(s: &str, end: usize) -> &str {
    s.get(..end).unwrap_or_default()
}
