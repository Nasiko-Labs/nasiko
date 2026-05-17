import { NextResponse } from "next/server";
import {
  fetchNasikoBackendHealthcheck,
  fetchNasikoGatewayHealth,
  getNasikoApiUrl,
  NasikoApiError,
} from "@/lib/nasiko";

/**
 * B2 smoke test: confirms Next server can reach Nasiko via NASIKO_API_URL.
 * GET /api/health — no user auth required.
 */
export async function GET() {
  const nasikoApiUrl = getNasikoApiUrl();

  try {
    const [gateway, backend] = await Promise.all([
      fetchNasikoGatewayHealth(),
      fetchNasikoBackendHealthcheck(),
    ]);

    return NextResponse.json({
      ok: true,
      nasikoApiUrl,
      gateway,
      backend,
    });
  } catch (err) {
    if (err instanceof NasikoApiError) {
      return NextResponse.json(
        {
          ok: false,
          nasikoApiUrl,
          error: err.message,
          status: err.status,
          body: err.body,
        },
        { status: 502 },
      );
    }

    const message = err instanceof Error ? err.message : "Unknown error";
    return NextResponse.json(
      { ok: false, nasikoApiUrl, error: message },
      { status: 500 },
    );
  }
}
