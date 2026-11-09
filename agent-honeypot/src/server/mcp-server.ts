/**
 * Honeypot as a REAL MCP server (streamable-HTTP JSON-RPC).
 *
 * This is what makes the deep Nasiko integration real: instead of our runner
 * calling the honeypot in-process, Nasiko's MCP Gateway calls THIS server over
 * HTTP after `permissions.rs` has authorized the tool. It speaks exactly the
 * three methods the gateway's `GenericMcpProvider` uses:
 *
 *   - `initialize`   → capabilities handshake (protocol 2025-06-18)
 *   - `tools/list`   → the 8 honeypot tools with JSON input schemas
 *   - `tools/call`   → execute a (fake, isolated) tool, return MCP content
 *
 * The gateway accepts either an `application/json` or a `text/event-stream`
 * response; we reply with plain JSON (stateless), which the gateway handles on
 * the first attempt with no session negotiation.
 *
 * SAFETY: unchanged from the in-process honeypot — every tool is simulated.
 */
import "./env.js";
import cors from "cors";
import express from "express";
import { HONEYPOT_TOOLS, listHoneypotTools } from "./honeypot.js";
import { TOOL_SCHEMAS } from "./tool-schemas.js";

const MCP_PROTOCOL_VERSION = "2025-06-18";

const app = express();
app.use(cors());
app.use(express.json());

const PORT = Number(process.env.HONEYPOT_MCP_PORT ?? 8799);

function rpcResult(id: unknown, result: unknown) {
  return { jsonrpc: "2.0", id, result };
}
function rpcError(id: unknown, code: number, message: string) {
  return { jsonrpc: "2.0", id, error: { code, message } };
}

/** The MCP endpoint the Nasiko connector points at: POST /mcp. */
app.post("/mcp", (req, res) => {
  const { id, method, params } = req.body ?? {};

  // Notifications (no id) — accept and return empty.
  if (id === undefined || id === null) {
    return res.json({});
  }

  switch (method) {
    case "initialize":
      return res.json(
        rpcResult(id, {
          protocolVersion: MCP_PROTOCOL_VERSION,
          serverInfo: { name: "agent-honeypot", version: "0.1.0" },
          capabilities: { tools: {} },
        }),
      );

    case "tools/list":
      return res.json(
        rpcResult(id, {
          tools: listHoneypotTools().map((name) => ({
            name,
            description: TOOL_SCHEMAS[name]?.description ?? name,
            inputSchema: TOOL_SCHEMAS[name]?.inputSchema ?? {
              type: "object",
              properties: {},
            },
          })),
        }),
      );

    case "tools/call": {
      const name = params?.name as string;
      const args = (params?.arguments ?? {}) as Record<string, unknown>;
      const tool = HONEYPOT_TOOLS[name];
      if (!tool) {
        return res.json(rpcError(id, -32602, `unknown tool: ${name}`));
      }
      const exec = tool(args);
      console.log(
        `[honeypot-mcp] tools/call ${name} ← Nasiko gateway (allowed, forwarded)`,
      );
      // MCP tools return `content` blocks. We also attach the structured
      // telemetry as a second block so a caller that wants it can parse it.
      return res.json(
        rpcResult(id, {
          content: [
            {
              type: "text",
              text:
                typeof exec.result === "string"
                  ? exec.result
                  : JSON.stringify(exec.result),
            },
            {
              type: "text",
              text: JSON.stringify({ __honeypot_telemetry: exec.telemetry }),
            },
          ],
          isError: false,
        }),
      );
    }

    case "ping":
      return res.json(rpcResult(id, {}));

    default:
      return res.json(rpcError(id, -32601, `method not found: ${method}`));
  }
});

app.get("/health", (_req, res) => res.json({ ok: true }));

app.listen(PORT, () => {
  console.log(
    `🐝 Honeypot MCP server (streamable-HTTP) on http://localhost:${PORT}/mcp`,
  );
});
