//! `encode_tools` and `decode_tools`: the catalog-level round trip.

use crate::catalog::Catalog;
use crate::error::{Result, ToolCompactError};
use crate::parse::parse_tool;
use crate::types::{CompactTool, CompactTools, ToolDef};

/// Render a catalog as compact definitions.
///
/// Fails closed: if any tool uses an unsupported schema feature, has an invalid or duplicate
/// name, or exceeds a limit, the whole catalog is rejected so the caller keeps the native form.
pub fn encode_tools(tools: &[ToolDef]) -> Result<CompactTools> {
    let catalog = Catalog::compile(tools)?;
    let compact: Vec<CompactTool> = catalog
        .tools()
        .iter()
        .map(|t| CompactTool {
            name: t.def.name.clone(),
            line: t.line.clone(),
        })
        .collect();
    let definitions = compact
        .iter()
        .map(|t| t.line.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(CompactTools {
        definitions,
        tools: compact,
    })
}

/// Parse compact definitions back into tool definitions.
///
/// This reads [`CompactTools::definitions`], the actual encoded text, not a retained copy of the
/// input, so `decode_tools(&encode_tools(t)?)? == t` is a real statement about the notation.
pub fn decode_tools(compact: &CompactTools) -> Result<Vec<ToolDef>> {
    if compact.definitions.is_empty() {
        return Ok(Vec::new());
    }
    compact
        .definitions
        .split('\n')
        .enumerate()
        .map(|(i, line)| {
            parse_tool(line).map_err(|e| {
                ToolCompactError::catalog(format!(
                    "line {} does not parse at byte {}: {}",
                    i + 1,
                    e.pos,
                    e.reason
                ))
            })
        })
        .collect()
}
