import { NextResponse } from "next/server";
import { getBearerAuthForNasiko, unauthorizedResponse } from "@/lib/auth";
import { filterNasikoSessionsInRange } from "@/lib/session-filters";
import { parseTimeRangeQuery } from "@/lib/time-range";
import { fetchNasikoObservabilitySessions, NasikoApiError } from "@/lib/nasiko";
import { normalizeSessions } from "@/lib/sessions";

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

  try {
    const sessionsResponse = await fetchNasikoObservabilitySessions(
      authorization,
      range.startTimeIso,
    );
    const sessions = normalizeSessions(
      filterNasikoSessionsInRange(
        sessionsResponse.data?.sessions ?? [],
        range.startMs,
        range.endMs,
      ),
      agent,
    );

    return NextResponse.json({
      agent,
      window: range.window,
      start_time: range.startTimeIso,
      end_time: range.endTimeIso,
      sessions,
    });
  } catch (err) {
    if (err instanceof NasikoApiError) {
      return NextResponse.json(
        { error: err.message, agent, detail: err.body },
        { status: err.status >= 400 && err.status < 600 ? err.status : 502 },
      );
    }
    const message = err instanceof Error ? err.message : "Unknown error";
    return NextResponse.json({ error: message, agent }, { status: 500 });
  }
}
