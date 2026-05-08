// ============================================================
// API CLIENT — Real backend calls to localhost:8000
// Every query produces dynamic output. No static caching.
// ============================================================

const BASE = 'http://localhost:8000';

export async function callAnalyze({ query, tool }) {
  const res = await fetch(`${BASE}/api/analyze`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ query, tool }),
  });
  if (!res.ok) throw new Error(`API error: ${res.status}`);
  return res.json();
}

export async function checkHealth() {
  try {
    const res = await fetch(`${BASE}/api/health`, { signal: AbortSignal.timeout(2000) });
    return res.ok;
  } catch {
    return false;
  }
}
