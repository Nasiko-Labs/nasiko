import { NextResponse } from 'next/server';
import { HealthCheckResponse } from '@/lib/types';

export const runtime = 'nodejs';
export const dynamic = 'force-dynamic';

export async function GET() {
  const backendUrl =
    process.env.RAILWAY_API_URL?.trim() ||
    process.env.BACKEND_API_URL?.trim() ||
    process.env.CLASSIFIER_API_URL?.trim() ||
    'http://127.0.0.1:8081/classify';

  let backendHealthy = false;
  let backendStatus = 'offline';

  try {
    const healthUrl = backendUrl.replace(/\/classify\/?$/, '/health');
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), 2000);
    const res = await fetch(healthUrl, { signal: controller.signal, cache: 'no-store' });
    clearTimeout(timeout);
    if (res.ok) {
      backendHealthy = true;
      backendStatus = 'online';
    }
  } catch {
    backendStatus = 'unreachable';
  }

  return NextResponse.json<HealthCheckResponse>({
    configured: Boolean(backendUrl),
    railwayUrlConfigured: Boolean(backendUrl),
    environment: process.env.NODE_ENV || 'development',
    backendHealthy,
    backendStatus,
  });
}
