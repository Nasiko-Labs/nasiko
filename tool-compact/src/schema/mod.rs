mod analyze;
mod ast;
mod render;
mod validate;

pub use analyze::analyze_tools;
pub(crate) use analyze::safe_identifier;
pub use ast::{CanonicalTool, Property, SchemaKind, SchemaNode};
pub(crate) use render::render;
pub(crate) use validate::validate;

pub(crate) const MAX_DEPTH: usize = 64;
pub(crate) const MAX_TOOLS: usize = 4096;
pub(crate) const MAX_PROPERTIES: usize = 4096;
