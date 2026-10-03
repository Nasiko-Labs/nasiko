//! A compiled catalog: validated names, lowered schemas and rendered lines.
//!
//! Both encoding and decoding start here, so the two directions can never disagree about which
//! tools exist or what their schemas mean. Compilation enforces every catalog-level limit and the
//! round-trip self-check.

use std::collections::HashMap;

use crate::error::{Result, ToolCompactError};
use crate::lexeme::is_tool_name;
use crate::limits::{MAX_COMPACT_BYTES, MAX_SCHEMA_BYTES, MAX_TOOLS};
use crate::parse::parse_tool;
use crate::render::render_tool;
use crate::schema::{Budget, Node};
use crate::types::ToolDef;

pub(crate) struct CompiledTool {
    pub def: ToolDef,
    pub schema: Option<Node>,
    pub line: String,
}

pub(crate) struct Catalog {
    tools: Vec<CompiledTool>,
    index: HashMap<String, usize>,
}

impl Catalog {
    pub(crate) fn compile(defs: &[ToolDef]) -> Result<Self> {
        if defs.len() > MAX_TOOLS {
            return Err(ToolCompactError::limit("MAX_TOOLS", MAX_TOOLS));
        }
        let mut serialized = 0usize;
        for def in defs {
            serialized = serialized.saturating_add(
                serde_json::to_string(def)
                    .map(|s| s.len())
                    .unwrap_or(usize::MAX),
            );
            if serialized > MAX_SCHEMA_BYTES {
                return Err(ToolCompactError::limit(
                    "MAX_SCHEMA_BYTES",
                    MAX_SCHEMA_BYTES,
                ));
            }
        }

        let mut index = HashMap::with_capacity(defs.len());
        let mut tools = Vec::with_capacity(defs.len());
        let mut budget = Budget::default();
        let mut compact_bytes = 0usize;
        for (i, def) in defs.iter().enumerate() {
            if !is_tool_name(&def.name) {
                return Err(ToolCompactError::catalog(format!(
                    "tool name {:?} is not valid (1-64 chars of [A-Za-z0-9_.-])",
                    def.name
                )));
            }
            if index.insert(def.name.clone(), i).is_some() {
                return Err(ToolCompactError::catalog(format!(
                    "tool name `{}` appears more than once",
                    def.name
                )));
            }
            let schema = def
                .parameters
                .as_ref()
                .map(|p| Node::from_value(&def.name, p, &mut budget))
                .transpose()?;
            let line = render_tool(def, schema.as_ref());
            compact_bytes = compact_bytes.saturating_add(line.len() + 1);
            if compact_bytes > MAX_COMPACT_BYTES {
                return Err(ToolCompactError::limit(
                    "MAX_COMPACT_BYTES",
                    MAX_COMPACT_BYTES,
                ));
            }
            // Round-trip self-check: the rendered line must parse back to exactly this tool.
            match parse_tool(&line) {
                Ok(back) if back == *def => {}
                Ok(_) => {
                    return Err(ToolCompactError::unsupported(
                        &def.name,
                        "",
                        "schema does not survive the compact notation (non-canonical input)",
                    ));
                }
                Err(e) => {
                    return Err(ToolCompactError::unsupported(
                        &def.name,
                        "",
                        format!(
                            "rendered line does not parse at byte {}: {}",
                            e.pos, e.reason
                        ),
                    ));
                }
            }
            tools.push(CompiledTool {
                def: def.clone(),
                schema,
                line,
            });
        }
        Ok(Self { tools, index })
    }

    pub(crate) fn get(&self, name: &str) -> Option<&CompiledTool> {
        self.index.get(name).map(|&i| &self.tools[i])
    }

    pub(crate) fn tools(&self) -> &[CompiledTool] {
        &self.tools
    }
}
