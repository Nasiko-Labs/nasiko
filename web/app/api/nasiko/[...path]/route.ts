import { NextRequest, NextResponse } from 'next/server';

const API_BASE =
  process.env.NASIKO_API_URL?.replace(/\/$/, '') ?? 'http://localhost:9100/api/v1';

export async function GET(
  req: NextRequest,
  ctx: { params: Promise<{ path: string[] }> },
) {
  const { path } = await ctx.params;
  const auth = req.headers.get('authorization');
  if (!auth) {
    return NextResponse.json({ detail: 'Authorization required' }, { status: 401 });
  }

  const target = new URL(`${API_BASE}/${path.join('/')}`);
  req.nextUrl.searchParams.forEach((v, k) => target.searchParams.set(k, v));

  const upstream = await fetch(target.toString(), {
    headers: { Authorization: auth, Accept: 'application/json' },
    cache: 'no-store',
  });

  const text = await upstream.text();
  return new NextResponse(text, {
    status: upstream.status,
    headers: { 'Content-Type': upstream.headers.get('content-type') ?? 'application/json' },
  });
}
