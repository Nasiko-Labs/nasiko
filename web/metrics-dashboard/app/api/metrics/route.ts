import { NextResponse } from "next/server";
import { getBearerAuthForNasiko, unauthorizedResponse } from "@/lib/auth";
import { emptyAgentMetrics } from "@/lib/metrics";
import { filterNasikoSessionsInRange } from "@/lib/session-filters";
import { parseTimeRangeQuery } from "@/lib/time-range";
import {
  fetchNasikoAgentStats,
  fetchNasikoObservabilitySessions,
  NasikoApiError,
} from "@/lib/nasiko";
import { buildAgentMetrics } from "@/lib/rollup";
import { recordAgentHealth } from "@/lib/uptime";
import type { NasikoObservabilitySession } from "@/lib/types";

/**
 * B4–B6: Nasiko stats + session rollup + Kong health uptime.
 * Query: `agent` (registry agentId), `window` (default `24h`).
 */
export async function GET(request: Request) {
  const authorization = getBearerAuthForNasiko(request);
  if (!authorization) {
    return unauthorizedResponse();
  }

  const { searchParams } = new URL(request.url);
  const agent = searchParams.get("agent")?.trim();
  if (!agent) {
    return NextResponse.json(
      { error: "Missing required query parameter: agent" },
      { status: 400 },
    );
  }

  const rangeResult = parseTimeRangeQuery(searchParams);
  if ("error" in rangeResult) {
    return NextResponse.json({ error: rangeResult.error }, { status: 400 });
  }
  const range = rangeResult;
  const startTimeIso = range.startTimeIso;

  try {
    const [statsResponse, sessionsResponse, uptimeResult] = await Promise.all([
      fetchNasikoAgentStats(authorization, agent, startTimeIso),
      fetchNasikoObservabilitySessions(authorization, startTimeIso).catch(
        () => ({ data: { sessions: [] as NasikoObservabilitySession[] } }),
      ),
      recordAgentHealth(agent),
    ]);

    const project = statsResponse.data?.project;
    if (!project) {
      return NextResponse.json(
        {
          error: "No observability data for agent",
          agent,
          detail: "Project stats empty — deploy the agent and generate traces.",
        },
        { status: 404 },
      );
    }

    const sessions = filterNasikoSessionsInRange(
      (sessionsResponse.data?.sessions ?? []).filter((s) => s.agent_id === agent),
      range.startMs,
      range.endMs,
    );

    const metrics = buildAgentMetrics({
      agentId: agent,
      window: range.window,
      hours: range.hours,
      project,
      sessions,
      uptimePct: uptimeResult.uptimePct,
      uptimeMeta: uptimeResult.meta,
      now: range.endMs,
    });

    return NextResponse.json(metrics);
  } catch (err) {
    if (err instanceof NasikoApiError) {
      let detail: unknown = err.body;
      try {
        detail = JSON.parse(err.body);
      } catch {
        /* keep raw body */
      }

      if (err.status === 404) {
        const detailStr =
          typeof detail === "object" &&
          detail !== null &&
          "detail" in detail &&
          typeof (detail as { detail: unknown }).detail === "string"
            ? (detail as { detail: string }).detail
            : "";

        const projectMissing =
          detailStr.includes("not found") ||
          detailStr.includes("Project not found");

        if (projectMissing) {
          return NextResponse.json(
            emptyAgentMetrics(
              agent,
              range.window,
              "No Phoenix traces yet for this agent. Use the agent in the main UI (chat or A2A) once; Phoenix creates project '" +
                agent +
                "' on first span.",
            ),
          );
        }

        return NextResponse.json(
          {
            error: "Agent or Phoenix project not found",
            agent,
            detail,
          },
          { status: 404 },
        );
      }

      return NextResponse.json(
        { error: err.message, agent, detail },
        { status: err.status >= 400 && err.status < 600 ? err.status : 502 },
      );
    }

    const message = err instanceof Error ? err.message : "Unknown error";
    return NextResponse.json({ error: message, agent }, { status: 500 });
  }
}
