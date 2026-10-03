import { NextResponse } from 'next/server'

const adapterUrl = process.env.NASIKO_CLASSIFIER_URL ?? 'http://127.0.0.1:8090/api/classify'

type RequestBody = { query?: string; context?: string | null; simulate_failure?: boolean }

export async function POST(request: Request) {
  let body: RequestBody
  try {
    body = await request.json()
  } catch {
    return NextResponse.json({ error: 'Request body must be valid JSON.' }, { status: 400 })
  }
  if (!body.query?.trim()) return NextResponse.json({ error: 'query is required' }, { status: 400 })

  try {
    const response = await fetch(adapterUrl, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ query: body.query, context: body.context ?? null, simulate_failure: body.simulate_failure ?? false }),
      cache: 'no-store',
    })
    const payload = await response.json().catch(() => ({ error: 'Adapter returned a non-JSON response.' }))
    return NextResponse.json(response.ok ? payload : { error: payload.error ?? `Adapter returned HTTP ${response.status}.` }, { status: response.status })
  } catch {
    return NextResponse.json({ error: `Nasiko adapter unavailable at ${adapterUrl}. Start the Rust adapter and try again.` }, { status: 503 })
  }
}

export async function GET() {
  return NextResponse.json({ adapter_url: adapterUrl, status: 'ready' })
}
