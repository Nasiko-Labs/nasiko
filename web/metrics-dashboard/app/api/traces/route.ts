import { NextResponse } from "next/server";
import { getBearerAuthForNasiko, unauthorizedResponse } from "@/lib/auth";
import { computeStartTimeIso, parseMetricsWindow } from "@/lib/metrics";
import {
  fetchNasikoAgentStats,
  fetchNasikoTraceDetails,
  NasikoApiError,
} from "@/lib/nasiko";
import { parseTraceSpans } from "@/lib/trace-graph";
import { spansToPipelineGraph } from "@/lib/trace-pipeline";

export async function GET(request: Request) {
  const authorization = getBearerAuthForNasiko(request);
  if (!authorization) {
    return unauthorizedResponse();
  }

  const { searchParams } = new URL(request.url);
  const agent = searchParams.get("agent")?.trim();
  const traceId = searchParams.get("traceId")?.trim();
  const projectIdParam = searchParams.get("projectId")?.trim();

  if (!agent || !traceId) {
    return NextResponse.json(
      { error: "Missing required query parameters: agent, traceId" },
      { status: 400 },
    );
  }

  const windowResult = parseMetricsWindow(searchParams.get("window"));
  if ("error" in windowResult) {
    return NextResponse.json({ error: windowResult.error }, { status: 400 });
  }

  try {
    let projectId = projectIdParam ?? null;
    if (!projectId) {
      const startTimeIso = computeStartTimeIso(windowResult.hours);
      const stats = await fetchNasikoAgentStats(authorization, agent, startTimeIso);
      projectId = stats.data?.project?.id ?? null;
    }

    if (!projectId) {
      return NextResponse.json(
        {
          error: "Phoenix project id not found for agent",
          agent,
          detail: "Deploy agent and generate traces first.",
        },
        { status: 404 },
      );
    }

    const tracePayload = await fetchNasikoTraceDetails(
      authorization,
      projectId,
      traceId,
    );

    const roots = parseTraceSpans(tracePayload);
    const userPreview = searchParams.get("userPreview")?.trim() || null;
    const { nodes, edges, truncated } = spansToPipelineGraph(roots, {
      userPreview,
    });

    const payload = tracePayload as Record<string, unknown>;
    const data = (payload.data ?? payload) as Record<string, unknown>;
    const trace = (data.trace ?? {}) as Record<string, unknown>;

    return NextResponse.json({
      agent,
      trace_id: traceId,
      latency_ms:
        typeof trace.latency_ms === "number" ? trace.latency_ms : null,
      num_spans: nodes.length,
      total_spans:
        typeof trace.num_spans === "number" ? trace.num_spans : nodes.length,
      nodes,
      edges,
      truncated,
    });
  } catch (err) {
    if (err instanceof NasikoApiError) {
      return NextResponse.json(
        { error: err.message, agent, traceId, detail: err.body },
        { status: err.status >= 400 && err.status < 600 ? err.status : 502 },
      );
    }
    const message = err instanceof Error ? err.message : "Unknown error";
    return NextResponse.json({ error: message, agent, traceId }, { status: 500 });
  }
}
