/**
 * Agent Honeypot backend.
 *
 * Exposes the attack lab over HTTP + an SSE live event stream. The policy layer
 * is injected per-run (unprotected vs. Nasiko-protected), so the exact same
 * deterministic scenario produces different outcomes.
 */
import "./env.js";
import cors from "cors";
import express from "express";
import { PolicyEngine } from "./policy.js";
import { policyForMode } from "./policies.js";
import { runAttack, type PolicyLayer } from "./runner.js";
import { getScenario, SCENARIOS } from "./scenarios.js";
import { listHoneypotTools } from "./honeypot.js";
import { setupNasiko, type NasikoWiring } from "./nasiko-setup.js";
import { GatewayPolicyLayer } from "./gateway-policy.js";
import type {
  ApprovalRequest,
  RunMode,
  SecurityEvent,
} from "../shared/types.js";

const app = express();
app.use(cors());
app.use(express.json());

const PORT = Number(process.env.HONEYPOT_PORT ?? 8787);

// ── SSE clients + in-flight approvals ────────────────────────────────────────
const sseClients = new Set<express.Response>();
const pendingApprovals = new Map<
  string,
  { resolve: (approved: boolean) => void; req: ApprovalRequest }
>();

function broadcast(type: string, payload: unknown) {
  const data = `event: ${type}\ndata: ${JSON.stringify(payload)}\n\n`;
  for (const res of sseClients) res.write(data);
}

app.get("/api/stream", (req, res) => {
  res.setHeader("Content-Type", "text/event-stream");
  res.setHeader("Cache-Control", "no-cache");
  res.setHeader("Connection", "keep-alive");
  res.flushHeaders();
  res.write(`event: hello\ndata: ${JSON.stringify({ ok: true })}\n\n`);
  sseClients.add(res);
  req.on("close", () => sseClients.delete(res));
});

app.get("/api/scenarios", (_req, res) => {
  res.json({ scenarios: SCENARIOS, tools: listHoneypotTools() });
});

// ── Deep gateway integration state ───────────────────────────────────────────
let gatewayWiring: NasikoWiring | null = null;

/**
 * Wire the honeypot into the live Nasiko gateway (register connector, connect,
 * enable, set per-tool stances). Idempotent — safe to call repeatedly.
 */
app.post("/api/gateway/setup", async (_req, res) => {
  try {
    gatewayWiring = await setupNasiko();
    res.json({
      ok: true,
      connectorId: gatewayWiring.connectorId,
      agentId: gatewayWiring.agentId,
      prefix: gatewayWiring.prefix,
    });
  } catch (e) {
    res.status(500).json({ ok: false, error: (e as Error).message });
  }
});

app.get("/api/gateway/status", (_req, res) => {
  res.json({ wired: gatewayWiring !== null, wiring: gatewayWiring });
});

/** Human approves/rejects a pending `ask`. */
app.post("/api/approve", (req, res) => {
  const { eventId, approved } = req.body as {
    eventId: string;
    approved: boolean;
  };
  const pending = pendingApprovals.get(eventId);
  if (!pending) return res.status(404).json({ error: "no such approval" });
  pending.resolve(Boolean(approved));
  pendingApprovals.delete(eventId);
  res.json({ ok: true });
});

/**
 * Run a scenario. Body: { scenarioId, mode, autoApprove? }.
 * Streams each SecurityEvent over SSE and returns the full RunResult.
 */
app.post("/api/run", async (req, res) => {
  const { scenarioId, mode, autoApprove } = req.body as {
    scenarioId: string;
    mode: RunMode;
    autoApprove?: boolean;
  };

  const scenario = getScenario(scenarioId);
  if (!scenario) return res.status(404).json({ error: "unknown scenario" });

  let policy: PolicyLayer;
  if (mode === "gateway") {
    if (!gatewayWiring) {
      return res.status(409).json({
        error: "gateway not wired — POST /api/gateway/setup first",
      });
    }
    policy = new GatewayPolicyLayer(gatewayWiring);
  } else {
    policy = new PolicyEngine(policyForMode(mode));
  }
  broadcast("run-start", { scenarioId, mode, policy: policy.label });

  const result = await runAttack(scenario, mode, policy, {
    onEvent: (e: SecurityEvent) => broadcast("event", e),
    onApproval: async (reqA) => {
      if (autoApprove !== undefined) return autoApprove;
      broadcast("approval-request", reqA);
      return new Promise<boolean>((resolve) => {
        pendingApprovals.set(reqA.eventId, { resolve, req: reqA });
        // Safety timeout: default reject after 60s so a run never hangs.
        setTimeout(() => {
          if (pendingApprovals.has(reqA.eventId)) {
            pendingApprovals.delete(reqA.eventId);
            resolve(false);
          }
        }, 60_000);
      });
    },
  });

  broadcast("run-complete", result);
  res.json(result);
});

app.listen(PORT, () => {
  console.log(
    `🐝 Agent Honeypot backend listening on http://localhost:${PORT}`,
  );
});
