import type {
  TraceGraphEdge,
  TraceGraphNode,
  TraceSpanNode,
} from "@/lib/types";

const MAX_SPANS = 60;

function spanNodeId(span: TraceSpanNode, fallback: string): string {
  return span.span_id ?? span.id ?? fallback;
}

function statusTone(status: string | null | undefined): string {
  const code = (status ?? "").toUpperCase();
  if (code === "ERROR" || code === "STATUS_CODE_ERROR") {
    return "error";
  }
  if (code === "OK" || code === "STATUS_CODE_OK") {
    return "ok";
  }
  return "neutral";
}

type WalkState = {
  nodes: TraceGraphNode[];
  edges: TraceGraphEdge[];
  row: number;
  count: number;
  maxSpans: number;
  truncated: boolean;
};

function walkSpan(
  span: TraceSpanNode,
  depth: number,
  state: WalkState,
  parentId?: string,
): void {
  if (state.count >= state.maxSpans) {
    state.truncated = true;
    return;
  }

  const id = spanNodeId(span, `span-${state.count}`);
  state.count += 1;
  state.row += 1;

  state.nodes.push({
    id,
    position: { x: depth * 240, y: state.row * 72 },
    data: {
      label: span.name?.trim() || span.span_kind || "span",
      latency_ms:
        span.latency_ms != null && Number.isFinite(span.latency_ms)
          ? span.latency_ms
          : null,
      status_code: span.status_code ?? null,
      span_kind: span.span_kind ?? null,
    },
  });

  if (parentId) {
    state.edges.push({
      id: `${parentId}->${id}`,
      source: parentId,
      target: id,
    });
  }

  for (const child of span.children ?? []) {
    walkSpan(child, depth + 1, state, id);
    if (state.truncated) {
      break;
    }
  }
}

export function spansToFlowGraph(
  roots: TraceSpanNode[],
  maxSpans = MAX_SPANS,
): { nodes: TraceGraphNode[]; edges: TraceGraphEdge[]; truncated: boolean } {
  const state: WalkState = {
    nodes: [],
    edges: [],
    row: 0,
    count: 0,
    maxSpans,
    truncated: false,
  };

  for (const root of roots) {
    walkSpan(root, 0, state);
    if (state.count >= maxSpans) {
      state.truncated = true;
      break;
    }
  }

  return {
    nodes: state.nodes,
    edges: state.edges,
    truncated: state.truncated,
  };
}

export function parseTraceSpans(tracePayload: unknown): TraceSpanNode[] {
  if (!tracePayload || typeof tracePayload !== "object") {
    return [];
  }
  const data = tracePayload as Record<string, unknown>;
  const inner = data.data as Record<string, unknown> | undefined;
  const trace = (inner?.trace ?? data.trace) as Record<string, unknown> | undefined;
  if (!trace) {
    return [];
  }
  const spans = trace.spans;
  if (Array.isArray(spans)) {
    return spans as TraceSpanNode[];
  }
  return [];
}

export { statusTone, MAX_SPANS };
