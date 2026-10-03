import { NextRequest, NextResponse } from 'next/server';
import { normalizeClassifierResponse, generateDemoResponse } from '@/lib/classifier';
import { ApiResponse, NormalizedClassifierResult } from '@/lib/types';

export const runtime = 'nodejs';
export const dynamic = 'force-dynamic';

export async function POST(req: NextRequest) {
  const startTime = Date.now();

  try {
    const body = await req.json().catch(() => null);

    if (!body || typeof body !== 'object') {
      return NextResponse.json<ApiResponse>(
        {
          success: false,
          error: 'Invalid request body. Expected a JSON payload with a "query" field.',
        },
        { status: 400 }
      );
    }

    const { query, context, demo } = body as {
      query?: unknown;
      context?: unknown;
      demo?: unknown;
    };

    if (typeof query !== 'string' || !query.trim()) {
      return NextResponse.json<ApiResponse>(
        {
          success: false,
          error: 'The "query" field is required and must not be empty.',
        },
        { status: 422 }
      );
    }

    const cleanQuery = query.trim();
    const cleanContext = typeof context === 'string' && context.trim() ? context.trim() : undefined;

    // Explicit demo mode requested by user
    if (demo === true) {
      const demoResult = generateDemoResponse(cleanQuery, cleanContext);
      return NextResponse.json<ApiResponse<NormalizedClassifierResult>>({
        success: true,
        data: demoResult,
      });
    }

    const railwayApiUrl = process.env.RAILWAY_API_URL?.trim();
    const railwayApiToken = process.env.RAILWAY_API_TOKEN?.trim();

    if (!railwayApiUrl) {
      return NextResponse.json<ApiResponse>(
        {
          success: false,
          error: 'Classifier backend is not configured.',
          details:
            'The RAILWAY_API_URL environment variable is missing on this server. Set RAILWAY_API_URL in .env.local (or in Vercel project environment variables) pointing to your Railway classifier endpoint.',
        },
        { status: 503 }
      );
    }

    // Forward request to Railway classifier backend
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), 12000);

    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      Accept: 'application/json',
    };

    if (railwayApiToken) {
      headers.Authorization = `Bearer ${railwayApiToken}`;
    }

    const upstreamPayload = {
      query: cleanQuery,
      context: cleanContext,
    };

    let upstreamResponse: Response;
    try {
      upstreamResponse = await fetch(railwayApiUrl, {
        method: 'POST',
        headers,
        body: JSON.stringify(upstreamPayload),
        signal: controller.signal,
        cache: 'no-store',
      });
    } catch (networkError: unknown) {
      clearTimeout(timeoutId);
      const isAbort = (networkError as Error)?.name === 'AbortError';
      const details = isAbort
        ? 'Request timed out after 12 seconds waiting for the Railway classifier.'
        : `Could not reach Railway backend at ${railwayApiUrl}: ${(networkError as Error)?.message || 'Connection failed'}`;

      return NextResponse.json<ApiResponse>(
        {
          success: false,
          error: isAbort ? 'Upstream classifier timed out.' : 'Failed to reach Railway classifier.',
          details,
        },
        { status: 504 }
      );
    }

    clearTimeout(timeoutId);
    const measuredLatencyMs = Date.now() - startTime;

    if (!upstreamResponse.ok) {
      const errorText = await upstreamResponse.text().catch(() => '');
      let parsedError: string | null = null;
      try {
        const jsonError = JSON.parse(errorText);
        parsedError = jsonError.error || jsonError.message || jsonError.detail;
      } catch {
        // Raw text error
      }

      return NextResponse.json<ApiResponse>(
        {
          success: false,
          error: `Railway backend returned HTTP ${upstreamResponse.status} (${upstreamResponse.statusText})`,
          details: parsedError || errorText || 'No diagnostic error body was returned from the classifier.',
          statusCode: upstreamResponse.status,
        },
        { status: 502 }
      );
    }

    let rawData: unknown;
    try {
      rawData = await upstreamResponse.json();
    } catch {
      return NextResponse.json<ApiResponse>(
        {
          success: false,
          error: 'Malformed response from classifier backend.',
          details: 'The Railway classifier responded with non-JSON content.',
        },
        { status: 502 }
      );
    }

    const normalized = normalizeClassifierResponse(rawData, measuredLatencyMs);

    return NextResponse.json<ApiResponse<NormalizedClassifierResult>>({
      success: true,
      data: normalized,
    });
  } catch (err: unknown) {
    return NextResponse.json<ApiResponse>(
      {
        success: false,
        error: 'An internal proxy error occurred while classifying request.',
        details: (err as Error)?.message || 'Unknown error',
      },
      { status: 500 }
    );
  }
}
