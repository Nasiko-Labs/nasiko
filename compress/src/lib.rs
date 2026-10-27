//! Structural, lossy-to-the-model compression of LLM payloads.
//!
//! Detects the shape of a payload (JSON, logs, diffs, search results) and removes the parts a
//! model is least likely to need, leaving a counted breadcrumb at every elision so the model
//! knows the data is incomplete.
//!
//! # Invariants
//!
//! [`compress`] is the only compressing entry point, and it is where every invariant is
//! enforced — the individual compressors are private and cannot bypass them.
//!
//! * **Fail-closed** — any failure returns the input unchanged. [`compress`] is infallible.
//! * **Never grows** — output strictly shorter than input, or the input comes back.
//! * **Idempotent** — input already carrying an elision marker is returned unchanged.
//! * **UTF-8 safe** — all truncation goes through [`text`], the only module permitted to slice.
//! * **Deterministic** — pure string processing. No clock, no RNG, no I/O, no model call.
//! * **Bounded** — input above [`Policy::max_input_bytes`] is head/tail truncated unparsed.
//!
//! # Config
//!
//! This crate never reads the environment. [`Policy`] is built by the caller;
//! [`TypeMask::from_labels`] and [`Level::parse`] exist so a caller holding config *strings*
//! does not reimplement the parsing.
//!
//! # Prior art
//!
//! The content-type taxonomy and the "compress at the egress proxy, leave a recoverable
//! breadcrumb" shape are Caveman's ideas (<https://github.com/juliusbrussee/caveman>). This is an
//! independent implementation from the published behaviour description; no Caveman code is used
//! or vendored — its engine is BSL-1.1.

#![forbid(unsafe_code)]
// I4 is a compiler rule here, not a review rule: `text` is the only module allowed to slice, and
// it slices on char boundaries. I1 forbids the panicking escape hatches outright.
#![deny(
    clippy::string_slice,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

mod compress;
mod compressed;
mod compressors;
mod content_type;
mod detect;
mod error;
mod levels;
mod marker;
mod policy;
mod table;
mod text;

pub use compress::compress;
pub use compressed::Compressed;
pub use content_type::{ContentType, TypeMask};
pub use detect::detect;
pub use error::CompressError;
pub use marker::contains_marker;
pub use policy::{Level, Policy};
pub use table::expand_tables;
