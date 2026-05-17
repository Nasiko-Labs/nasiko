import { NextResponse } from "next/server";
import { getBearerAuthForNasiko, unauthorizedResponse } from "@/lib/auth";
import { fetchNasikoUserAgents, NasikoApiError } from "@/lib/nasiko";
import type { MetricsAgentOption } from "@/lib/types";

/**
 * Proxy to Nasiko GET /api/v1/registry/user/agents for the agent selector (B3).
 */
export async function GET(request: Request) {
  const authorization = getBearerAuthForNasiko(request);
  if (!authorization) {
    return unauthorizedResponse();
  }

  try {
    const nasikoResponse = await fetchNasikoUserAgents(authorization);

    const agents: MetricsAgentOption[] = (nasikoResponse.data ?? []).map(
      (agent) => ({
        agentId: agent.agent_id,
        name: agent.name,
        description: agent.description ?? null,
        tags: agent.tags ?? [],
      }),
    );

    return NextResponse.json({
      agents,
      statusCode: nasikoResponse.status_code,
      message: nasikoResponse.message,
    });
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
