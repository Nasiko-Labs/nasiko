//! Every bound the crate enforces, in one place.
//!
//! Each constant is checked at a specific site (named in its doc) and exceeding it yields
//! [`crate::ToolCompactError::LimitExceeded`] with the constant's name as `limit`. Limits are
//! checked before the allocation or recursion they protect wherever the input makes that
//! possible; the few that cannot be (node count, rendered size) abort as soon as the running
//! total crosses the bound.

/// Tools per catalog (`encode_tools`, `StreamDecoder::new`).
pub const MAX_TOOLS: usize = 128;
/// Serialized size of all tool definitions handed to `encode_tools`, checked before lowering.
pub const MAX_SCHEMA_BYTES: usize = 256 * 1024;
/// Schema nodes (objects, arrays, scalars) across one catalog, counted while lowering.
pub const MAX_SCHEMA_NODES: usize = 4096;
/// Nesting depth of one tool's parameter schema.
pub const MAX_SCHEMA_DEPTH: usize = 32;
/// Properties in one object schema.
pub const MAX_PROPERTIES: usize = 256;
/// Members in one `enum`.
pub const MAX_ENUM_MEMBERS: usize = 256;
/// Bytes of one `description`, `title` or serialized `default`/`examples`.
pub const MAX_DESCRIPTION_BYTES: usize = 4096;
/// Rendered size of the whole compact catalog.
pub const MAX_COMPACT_BYTES: usize = 256 * 1024;
/// Bytes of a tool or property name.
pub const MAX_NAME_LEN: usize = 64;

/// Calls in one model reply.
pub const MAX_CALLS: usize = 32;
/// Bytes of one call's JSON arguments.
pub const MAX_ARGS_BYTES: usize = 256 * 1024;
/// Bytes of JSON arguments summed over all calls in one reply.
pub const MAX_TOTAL_ARGS_BYTES: usize = 512 * 1024;
/// Nesting depth of a call's JSON arguments.
pub const MAX_DEPTH: usize = 32;
/// Bytes pushed into one `StreamDecoder` in total, prose included.
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
