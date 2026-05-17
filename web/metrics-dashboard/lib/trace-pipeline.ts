import type {
  TraceGraphEdge,
  TraceGraphNode,
  TraceSpanNode,
} from "@/lib/types";

export type WorkflowNodeKind = "user" | "llm" | "agent" | "tool" | "step";

export type WorkflowNodeTone = "ok" | "error" | "neutral";

const NODE_GAP_X = 240;

/** Internal plumbing — hide from the n8n-style canvas. */
export function isInfrastructureSpan(span: TraceSpanNode): boolean {
  const name = (span.name ?? "").toLowerCase();
  return (
    /jsonrpc|event_consumer|request_handler|request_handlers|callback|middleware|\.on_message/i.test(
      name,
    ) || /a2a\.server\.|fastapi|starlette|uvicorn|http\.route/i.test(name)
  );
}

export function classifySpan(span: TraceSpanNode): WorkflowNodeKind {
  if (isInfrastructureSpan(span)) {
    return "step";
  }

  const name = (span.name ?? "").toLowerCase();
  const kind = (span.span_kind ?? "").toUpperCase();

  if (
    kind === "LLM" ||
    /openai|gpt-|chat\.?completion|chatcompletion|langchain\.chat|llm/i.test(name)
  ) {
    return "llm";
  }
  if (kind === "TOOL" || /tool|function_call|mcp/i.test(name)) {
    return "tool";
  }
  if (
    kind === "CHAIN" ||
    kind === "AGENT" ||
    (/agent|chain|invoke|orchestrat/i.test(name) && !/handler/i.test(name))
  ) {
    return "agent";
  }
  if (/^user|^human|user_message|human_message/i.test(name)) {
    return "user";
  }
  return "step";
}

export function spanTone(status: string | null | undefined): WorkflowNodeTone {
  const code = (status ?? "").toUpperCase();
  if (code === "ERROR" || code === "STATUS_CODE_ERROR") {
    return "error";
  }
  if (code === "OK" || code === "STATUS_CODE_OK") {
    return "ok";
  }
  return "neutral";
}

function flattenSpans(roots: TraceSpanNode[]): TraceSpanNode[] {
  const out: TraceSpanNode[] = [];
  const walk = (span: TraceSpanNode) => {
    out.push(span);
    for (const child of span.children ?? []) {
      walk(child);
    }
  };
  for (const root of roots) {
    walk(root);
  }
  return out;
}

function pickBestSpan(
  flat: TraceSpanNode[],
  kind: WorkflowNodeKind,
): TraceSpanNode | null {
  const candidates = flat.filter(
    (s) => classifySpan(s) === kind && !isInfrastructureSpan(s),
  );
  if (candidates.length === 0) {
    return null;
  }
  return [...candidates].sort(
    (a, b) => (b.latency_ms ?? 0) - (a.latency_ms ?? 0),
  )[0];
}

export function shortLlmSubtitle(name: string | undefined): string {
  const n = (name ?? "").trim();
  if (!n) return "Model call";
  if (/chatcompletion/i.test(n)) return "Chat completion";
  if (/completion/i.test(n)) return "Completion";
  const short = n.split(".").pop() ?? n;
  return short.length > 28 ? `${short.slice(0, 25)}…` : short;
}

function spanSubtitle(
  span: TraceSpanNode,
  kind: WorkflowNodeKind,
): string | undefined {
  if (kind === "llm") {
    return shortLlmSubtitle(span.name);
  }
  if (kind === "agent") {
    return "Agent orchestration";
  }
  if (kind === "tool") {
    const n = span.name?.trim();
    return n ? (n.length > 32 ? `${n.slice(0, 29)}…` : n) : "Tool call";
  }
  const sk = span.span_kind?.trim();
  if (!sk || sk.toLowerCase() === "unknown") {
    return undefined;
  }
  return sk;
}

function createUserNode(preview: string | null | undefined): TraceGraphNode {
  const text = preview?.trim();
  return {
    id: "pipeline-user",
    position: { x: 0, y: 48 },
    data: {
      label: text
        ? text.length > 42
          ? `${text.slice(0, 39)}…`
          : text
        : "User prompt",
      latency_ms: null,
      status_code: "OK",
      span_kind: "USER",
      node_kind: "user",
      tone: "ok",
      subtitle: "Incoming message",
    },
  };
}

function createStepNode(
  span: TraceSpanNode,
  kind: WorkflowNodeKind,
  index: number,
): TraceGraphNode {
  const tone = spanTone(span.status_code);
  const label =
    kind === "llm"
      ? "OpenAI"
      : kind === "agent"
        ? "Agent"
        : kind === "tool"
          ? "Tool"
          : "Step";

  return {
    id: span.span_id ?? span.id ?? `pipeline-${kind}-${index}`,
    position: { x: index * NODE_GAP_X, y: 48 },
    data: {
      label,
      latency_ms:
        span.latency_ms != null && Number.isFinite(span.latency_ms)
          ? span.latency_ms
          : null,
      status_code: span.status_code ?? null,
      span_kind: span.span_kind ?? null,
      node_kind: kind,
      tone,
      subtitle: spanSubtitle(span, kind),
    },
  };
}

/**
 * Curated left-to-right pipeline: User prompt → OpenAI → (optional Agent/Tool).
 * Skips JSON-RPC handlers, callbacks, and other internal spans.
 */
export function spansToPipelineGraph(
  roots: TraceSpanNode[],
  options?: { userPreview?: string | null; maxSteps?: number },
): { nodes: TraceGraphNode[]; edges: TraceGraphEdge[]; truncated: boolean } {
  const flat = flattenSpans(roots);
  const truncated = flat.length > (options?.maxSteps ?? 14);

  const nodes: TraceGraphNode[] = [createUserNode(options?.userPreview)];

  const llm = pickBestSpan(flat, "llm");
  if (llm) {
    nodes.push(createStepNode(llm, "llm", nodes.length));
  }

  const agent = pickBestSpan(flat, "agent");
  if (agent && agent !== llm) {
    nodes.push(createStepNode(agent, "agent", nodes.length));
  }

  const tool = pickBestSpan(flat, "tool");
  if (tool && tool !== llm && tool !== agent) {
    nodes.push(createStepNode(tool, "tool", nodes.length));
  }

  const anyError = flat.some(
    (s) =>
      spanTone(s.status_code) === "error" && classifySpan(s) !== "step",
  );
  if (anyError && nodes[nodes.length - 1]?.data.tone !== "error") {
    const failed = flat.find(
      (s) =>
        spanTone(s.status_code) === "error" &&
        (classifySpan(s) === "llm" ||
          classifySpan(s) === "agent" ||
          classifySpan(s) === "tool"),
    );
    if (failed) {
      const kind = classifySpan(failed);
      const existing = nodes.find((n) => n.data.node_kind === kind);
      if (existing) {
        existing.data.tone = "error";
      }
    } else {
      nodes[nodes.length - 1].data.tone = "error";
    }
  }

  const edges: TraceGraphEdge[] = [];
  for (let i = 0; i < nodes.length; i++) {
    nodes[i].position = { x: i * NODE_GAP_X, y: 48 };
    if (i > 0) {
      edges.push({
        id: `edge-${i}`,
        source: nodes[i - 1].id,
        target: nodes[i].id,
      });
    }
  }

  return { nodes, edges, truncated };
}

export function defaultLabel(kind: WorkflowNodeKind): string {
  switch (kind) {
    case "user":
      return "User prompt";
    case "llm":
      return "OpenAI";
    case "agent":
      return "Agent";
    case "tool":
      return "Tool";
    default:
      return "Step";
  }
}
