/**
 * Challenge 1 — GET /api/logs
 *
 * Synthesizes a "platform activity log" from existing Nasiko observability
 * sessions + registry data. No Nasiko API changes; no Docker access; runs
 * anywhere the dashboard runs.
 *
 * Query params:
 *   window=24h   (default; only 24h is supported today)
 */

import { NextResponse } from "next/server";
import { getBearerAuthForNasiko, unauthorizedResponse } from "@/lib/auth";
import { computeStartTimeIso, parseMetricsWindow } from "@/lib/metrics";
import {
  fetchNasikoObservabilitySessions,
  fetchNasikoUserAgents,
  NasikoApiError,
} from "@/lib/nasiko";
import { synthesizeLogs } from "@/lib/logs";
import type { LogsResponse } from "@/lib/types";

export async function GET(request: Request) {
  const authorization = getBearerAuthForNasiko(request);
  if (!authorization) {
    return unauthorizedResponse();
  }

  const { searchParams } = new URL(request.url);
  const windowResult = parseMetricsWindow(searchParams.get("window"));
  if ("error" in windowResult) {
    return NextResponse.json({ error: windowResult.error }, { status: 400 });
  }

  const startTimeIso = computeStartTimeIso(windowResult.hours);
  const now = new Date();

  try {
    const [agentsResponse, sessionsResponse] = await Promise.all([
      fetchNasikoUserAgents(authorization).catch((err) => {
        // Registry is best-effort — if it fails, we still ship session rows.
        if (err instanceof NasikoApiError && err.status === 401) {
          throw err;
        }
        return { data: [], status_code: 0, message: "" };
      }),
      fetchNasikoObservabilitySessions(authorization, startTimeIso).catch(
        (err) => {
          if (err instanceof NasikoApiError && err.status === 401) {
            throw err;
          }
          return { data: { sessions: [] } };
        },
      ),
    ]);

    const sessions = sessionsResponse.data?.sessions ?? [];
    const agents = agentsResponse.data ?? [];

    const { logs, source_counts } = synthesizeLogs({
      sessions,
      agents,
      generatedAt: now,
    });

    const body: LogsResponse = {
      window: windowResult.window,
      generated_at: now.toISOString(),
      total: logs.length,
      source_counts,
      logs,
    };

    return NextResponse.json(body);
  } catch (err) {
    if (err instanceof NasikoApiError) {
      let detail: unknown = err.body;
      try {
        detail = JSON.parse(err.body);
      } catch {
        /* keep raw body */
      }
      return NextResponse.json(
        { error: err.message, detail },
        { status: err.status >= 400 && err.status < 600 ? err.status : 502 },
      );
    }
    const message = err instanceof Error ? err.message : "Unknown error";
    return NextResponse.json({ error: message }, { status: 500 });
  }
}
