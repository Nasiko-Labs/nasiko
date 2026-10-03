//! Shared code for the `compact_tools_eval` and `compact_tools_measure` examples.
//!
//! Nothing here is production code: it reads evaluation sets, drives the router's
//! `compact_tools` transformation exactly as the handler does, writes JSONL, and (in live mode)
//! talks to one OpenAI-compatible endpoint. The library crate stays free of all of it.

#![allow(dead_code)]

pub mod eval;
pub mod live;
pub mod measure;
