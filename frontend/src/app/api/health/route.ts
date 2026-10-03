import { NextResponse } from 'next/server';
import { HealthCheckResponse } from '@/lib/types';

export const runtime = 'nodejs';
export const dynamic = 'force-dynamic';

export async function GET() {
  const railwayUrlConfigured = Boolean(process.env.RAILWAY_API_URL?.trim());

  return NextResponse.json<HealthCheckResponse>({
    configured: railwayUrlConfigured,
    railwayUrlConfigured,
    environment: process.env.NODE_ENV || 'development',
  });
}
