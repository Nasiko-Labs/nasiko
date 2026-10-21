import type {
  ApprovalRequest,
  AttackScenario,
  RunMode,
  RunResult,
  SecurityEvent,
} from "../shared/types.js";

const BASE = "/api";

export async function fetchScenarios(): Promise<{
  scenarios: AttackScenario[];
  tools: string[];
}> {
  const r = await fetch(`${BASE}/scenarios`);
  return r.json();
}

export interface GatewayStatus {
  wired: boolean;
  wiring: { connectorId: string; agentId: string; prefix: string } | null;
}

export async function gatewayStatus(): Promise<GatewayStatus> {
  const r = await fetch(`${BASE}/gateway/status`);
  return r.json();
}

export async function gatewaySetup(): Promise<{
  ok: boolean;
  error?: string;
  connectorId?: string;
  agentId?: string;
}> {
  const r = await fetch(`${BASE}/gateway/setup`, { method: "POST" });
  return r.json();
}

export async function runScenario(
  scenarioId: string,
  mode: RunMode,
  autoApprove?: boolean,
): Promise<RunResult> {
  const r = await fetch(`${BASE}/run`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ scenarioId, mode, autoApprove }),
  });
  return r.json();
}

export async function respondApproval(
  eventId: string,
  approved: boolean,
): Promise<void> {
  await fetch(`${BASE}/approve`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ eventId, approved }),
  });
}

export interface StreamHandlers {
  onEvent?: (e: SecurityEvent) => void;
  onRunStart?: (d: {
    scenarioId: string;
    mode: RunMode;
    policy: string;
  }) => void;
  onRunComplete?: (r: RunResult) => void;
  onApprovalRequest?: (r: ApprovalRequest) => void;
}

export function connectStream(handlers: StreamHandlers): () => void {
  const es = new EventSource(`${BASE}/stream`);
  es.addEventListener("event", (e) =>
    handlers.onEvent?.(JSON.parse((e as MessageEvent).data)),
  );
  es.addEventListener("run-start", (e) =>
    handlers.onRunStart?.(JSON.parse((e as MessageEvent).data)),
  );
  es.addEventListener("run-complete", (e) =>
    handlers.onRunComplete?.(JSON.parse((e as MessageEvent).data)),
  );
  es.addEventListener("approval-request", (e) =>
    handlers.onApprovalRequest?.(JSON.parse((e as MessageEvent).data)),
  );
  return () => es.close();
}
