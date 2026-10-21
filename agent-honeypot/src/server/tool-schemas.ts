/**
 * JSON input schemas for the honeypot tools. Used by:
 *   - the MCP server's `tools/list` (so Nasiko + any MCP client sees them)
 *   - (future) an LLM agent's function-calling definitions.
 */
export interface ToolSchema {
  description: string;
  inputSchema: {
    type: "object";
    properties: Record<string, unknown>;
    required?: string[];
  };
}

const s = (
  description: string,
  properties: Record<string, unknown>,
  required: string[] = [],
): ToolSchema => ({
  description,
  inputSchema: { type: "object", properties, required },
});

export const TOOL_SCHEMAS: Record<string, ToolSchema> = {
  "web.search": s("Search the web (simulated).", {
    query: { type: "string", description: "Search query" },
  }),
  "github.search": s("Search GitHub repos (simulated).", {
    query: { type: "string" },
  }),
  "filesystem.read": s(
    "Read a file from the app filesystem (simulated).",
    { path: { type: "string", description: "Absolute path, e.g. /app/.env" } },
    ["path"],
  ),
  "filesystem.write": s(
    "Write a file (simulated, sandbox only).",
    {
      path: { type: "string" },
      content: { type: "string" },
    },
    ["path"],
  ),
  "secrets.read": s("Read a secret from the secrets manager (simulated).", {
    key: { type: "string", description: "Secret name, or empty for all" },
  }),
  "database.query": s(
    "Run a read query against the app database (simulated).",
    { sql: { type: "string", description: "A SELECT statement" } },
    ["sql"],
  ),
  "database.delete": s(
    "Delete rows from a table (simulated — nothing is ever deleted).",
    { table: { type: "string" } },
    ["table"],
  ),
  "email.send": s(
    "Send an email (simulated — nothing is ever sent).",
    {
      to: { type: "string" },
      subject: { type: "string" },
      body: { type: "string" },
    },
    ["to"],
  ),
  "shell.execute": s(
    "Execute a shell command (simulated — fake interpreter only).",
    { command: { type: "string" } },
    ["command"],
  ),
};
