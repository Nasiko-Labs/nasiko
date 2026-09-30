//! Per-type compressors.
//!
//! Every one of these is private and returns `Option<String>` — `None` meaning "no saving found",
//! which deliberately does not distinguish a malformed payload from an incompressible one. The
//! caller is always [`crate::compress`], which owns the invariants.

mod diff;
mod json;
mod log;
mod search;

use crate::content_type::ContentType;
use crate::policy::Policy;

pub(crate) fn dispatch(kind: ContentType, input: &str, policy: &Policy<'_>) -> Option<String> {
    match kind {
        ContentType::Json => json::compress(input, policy),
        ContentType::Log => log::compress(input, policy),
        ContentType::Diff => diff::compress(input, policy),
        ContentType::SearchResults => search::compress(input, policy),
        // Variants exist so the enum and the config surface are stable; compressors land in a
        // later release once the eval suite shows they clear the never-grows bar on real inputs.
        ContentType::Code | ContentType::Markup | ContentType::Prose => None,
    }
}
