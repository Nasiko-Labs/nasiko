import { NextResponse } from "next/server";
import { getBearerAuthForNasiko, unauthorizedResponse } from "@/lib/auth";
import { computeStartTimeIso, parseMetricsWindow } from "@/lib/metrics";
import {
  fetchNasikoAgentStats,
  fetchNasikoSessionDetails,
  NasikoApiError,
} from "@/lib/nasiko";
import { extractSessionTraces } from "@/lib/sessions";

type RouteContext = { params: Promise<{ sessionId: string }> };

export async function GET(request: Request, context: RouteContext) {
  const authorization = getBearerAuthForNasiko(request);
  if (!authorization) {
    return unauthorizedResponse();
  }

  const { sessionId } = await context.params;
  const { searchParams } = new URL(request.url);
  const agent = searchParams.get("agent")?.trim();
  if (!agent) {
    return NextResponse.json(
      { error: "Missing required query parameter: agent" },
      { status: 400 },
    );
  }

  const windowResult = parseMetricsWindow(searchParams.get("window"));
  if ("error" in windowResult) {
    return NextResponse.json({ error: windowResult.error }, { status: 400 });
  }

  try {
    const startTimeIso = computeStartTimeIso(windowResult.hours);
    const [sessionPayload, statsResponse] = await Promise.all([
      fetchNasikoSessionDetails(authorization, sessionId),
      fetchNasikoAgentStats(authorization, agent, startTimeIso).catch(() => null),
    ]);

    const payload = sessionPayload as Record<string, unknown>;
    const data = (payload.data ?? payload) as Record<string, unknown>;
    const session = (data.session ?? data) as Record<string, unknown>;
    const tracesRaw = (session.traces ?? []) as Array<Record<string, unknown>>;
    const traces = extractSessionTraces(tracesRaw);
    const projectId =
      (statsResponse?.data?.project?.id as string | undefined) ?? null;

    return NextResponse.json({
      session_id: sessionId,
      num_traces: (session.num_traces as number | undefined) ?? traces.length,
      traces,
      project_id: projectId,
    });
  } catch (err) {
    if (err instanceof NasikoApiError) {
      return NextResponse.json(
        { error: err.message, session_id: sessionId, detail: err.body },
        { status: err.status >= 400 && err.status < 600 ? err.status : 502 },
      );
    }
    const message = err instanceof Error ? err.message : "Unknown error";
    return NextResponse.json({ error: message, session_id: sessionId }, { status: 500 });
  }
}
