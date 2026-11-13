/**
 * Persist evaluate runs into Nasiko chat_sessions so they appear on
 * http://localhost:8080/sessions.html (Execution history).
 *
 * Prefers the control-plane HTTP API when NASIKO_API_TOKEN (or login creds)
 * are set; otherwise falls back to a direct Postgres write when
 * NASIKO_HISTORY_DATABASE_URL is set (or the local docker default resolves).
 *
 * Never throws — history is best-effort and must not break /evaluate.
 */

import { randomUUID } from 'node:crypto'

const DEFAULT_CP = 'http://server:8080'
const DEFAULT_DB = 'postgres://nasiko:nasiko@postgres:5432/nasiko_dev'
const DEFAULT_AGENT_NAME = 'hackpay-git-eval'

function trim(v) {
  return typeof v === 'string' ? v.trim() : ''
}

function enabled() {
  const flag = trim(process.env.NASIKO_SESSION_HISTORY).toLowerCase()
  if (flag === '0' || flag === 'false' || flag === 'off') return false
  // On by default when we have any way to talk to Nasiko history storage.
  return true
}

function cpBase() {
  return trim(process.env.NASIKO_CP_URL) || trim(process.env.NASIKO_URL) || DEFAULT_CP
}

function titleFrom(idea, githubUrl) {
  const repo = String(githubUrl || '')
    .replace(/^https?:\/\/(www\.)?github\.com\//i, '')
    .replace(/\.git$/i, '')
    .split(/[?#]/)[0]
  const ideaBit = String(idea || '')
    .trim()
    .slice(0, 40)
  const raw = repo ? `Eval ${repo}${ideaBit ? ` — ${ideaBit}` : ''}` : ideaBit || 'Git eval'
  return raw.length > 80 ? `${raw.slice(0, 79)}…` : raw
}

function userPreview(idea, githubUrl) {
  return JSON.stringify(
    {
      event: 'repo_submission.evaluate',
      idea: String(idea || '').slice(0, 500),
      githubUrl,
    },
    null,
    2,
  )
}

function assistantPreview(result) {
  const a = result?.assessment
  if (!a?.scores) {
    return JSON.stringify(result, null, 2).slice(0, 8000)
  }
  return JSON.stringify(
    {
      ok: result.ok,
      source: result.source,
      scores: a.scores,
      overallFeedback: a.overallFeedback,
      strengths: a.strengths,
      improvements: a.improvements,
      evidence: a.evidence,
      provider: a.provider,
      assessedAt: a.assessedAt,
    },
    null,
    2,
  )
}

let cachedToken = null
let cachedTokenAt = 0

async function resolveToken() {
  const explicit = trim(process.env.NASIKO_API_TOKEN) || trim(process.env.NASIKO_GIT_EVAL_API_KEY)
  if (explicit) return explicit

  if (cachedToken && Date.now() - cachedTokenAt < 6 * 24 * 60 * 60 * 1000) {
    return cachedToken
  }

  const username = trim(process.env.NASIKO_USERNAME) || 'admin'
  const password = trim(process.env.NASIKO_PASSWORD) || trim(process.env.NASIKO_ADMIN_PASSWORD)
  if (!password) return null

  try {
    const res = await fetch(`${cpBase()}/api/auth/login`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ username, password }),
    })
    if (!res.ok) return null
    const body = await res.json()
    const token = body.token || body.data?.token || body.access_token
    if (token) {
      cachedToken = token
      cachedTokenAt = Date.now()
    }
    return token || null
  } catch {
    return null
  }
}

async function recordViaHttp({ sessionId, idea, githubUrl, result }) {
  const token = await resolveToken()
  if (!token) return { ok: false, reason: 'no-token' }

  const agentId =
    trim(process.env.NASIKO_AGENT_ID) || trim(process.env.NASIKO_AGENT_NAME) || DEFAULT_AGENT_NAME
  const base = cpBase()
  const headers = {
    'Content-Type': 'application/json',
    Authorization: `Bearer ${token}`,
  }

  const createRes = await fetch(`${base}/api/chat/sessions`, {
    method: 'POST',
    headers,
    body: JSON.stringify({
      session_id: sessionId,
      agent_id: agentId,
      first_prompt: titleFrom(idea, githubUrl),
    }),
  })
  if (!createRes.ok && createRes.status !== 409) {
    const text = await createRes.text().catch(() => '')
    return { ok: false, reason: `create ${createRes.status}: ${text.slice(0, 120)}` }
  }

  const userContent = userPreview(idea, githubUrl)
  const assistantContent = assistantPreview(result)

  for (const [role, content] of [
    ['user', userContent],
    ['assistant', assistantContent],
  ]) {
    const msgRes = await fetch(`${base}/api/chat/sessions/${encodeURIComponent(sessionId)}/messages`, {
      method: 'POST',
      headers,
      body: JSON.stringify({ role, content }),
    })
    if (!msgRes.ok) {
      const text = await msgRes.text().catch(() => '')
      return { ok: false, reason: `message ${role} ${msgRes.status}: ${text.slice(0, 120)}` }
    }
  }

  return { ok: true, via: 'http', sessionId }
}

async function loadPg() {
  try {
    const mod = await import('pg')
    return mod.default?.Client || mod.Client
  } catch {
    return null
  }
}

async function recordViaPostgres({ sessionId, idea, githubUrl, result }) {
  const Client = await loadPg()
  if (!Client) return { ok: false, reason: 'no-pg' }

  const databaseUrl = trim(process.env.NASIKO_HISTORY_DATABASE_URL) || DEFAULT_DB
  const client = new Client({ connectionString: databaseUrl, connectionTimeoutMillis: 2500 })
  try {
    await client.connect()
  } catch (err) {
    return {
      ok: false,
      reason: `db-connect: ${err instanceof Error ? err.message : 'failed'}`,
    }
  }

  try {
    const agentName = trim(process.env.NASIKO_AGENT_NAME) || DEFAULT_AGENT_NAME
    let agentId = trim(process.env.NASIKO_AGENT_ID) || null
    if (!agentId) {
      const row = await client.query(
        `SELECT id::text AS id FROM agents WHERE name = $1 AND deleted_at IS NULL LIMIT 1`,
        [agentName],
      )
      agentId = row.rows[0]?.id || null
    }

    let userId = trim(process.env.NASIKO_HISTORY_USER_ID) || null
    if (!userId) {
      const username = trim(process.env.NASIKO_USERNAME) || 'admin'
      const row = await client.query(`SELECT id::text AS id FROM users WHERE username = $1 LIMIT 1`, [
        username,
      ])
      userId = row.rows[0]?.id || null
    }
    if (!userId) {
      return { ok: false, reason: 'no-user' }
    }

    const title = titleFrom(idea, githubUrl)
    const agentUrl = agentId ? `/api/agents/${agentId}` : null

    await client.query(
      `INSERT INTO chat_sessions (session_id, user_id, agent_id, agent_url, title)
       VALUES ($1, $2::uuid, $3::uuid, $4, $5)
       ON CONFLICT (session_id) DO NOTHING`,
      [sessionId, userId, agentId, agentUrl, title],
    )

    await client.query(
      `INSERT INTO chat_messages (session_id, role, content) VALUES ($1, 'user', $2)`,
      [sessionId, userPreview(idea, githubUrl)],
    )
    await client.query(
      `INSERT INTO chat_messages (session_id, role, content) VALUES ($1, 'assistant', $2)`,
      [sessionId, assistantPreview(result)],
    )
    await client.query(`UPDATE chat_sessions SET updated_at = now() WHERE session_id = $1`, [
      sessionId,
    ])

    return { ok: true, via: 'postgres', sessionId }
  } catch (err) {
    return {
      ok: false,
      reason: `db-write: ${err instanceof Error ? err.message : 'failed'}`,
    }
  } finally {
    await client.end().catch(() => {})
  }
}

/**
 * Record one evaluate turn into Nasiko session history.
 * @returns {Promise<{ok:boolean, via?:string, sessionId?:string, reason?:string}>}
 */
export async function recordEvaluateSession({ idea, githubUrl, result }) {
  if (!enabled()) return { ok: false, reason: 'disabled' }

  const sessionId = `ses_eval_${randomUUID().replace(/-/g, '').slice(0, 24)}`

  // Prefer HTTP (works whenever the CP is reachable + auth is configured).
  const http = await recordViaHttp({ sessionId, idea, githubUrl, result }).catch((err) => ({
    ok: false,
    reason: err instanceof Error ? err.message : 'http-failed',
  }))
  if (http.ok) return http

  // Fall back to direct Postgres (local docker compose: agent ↔ postgres network).
  const pg = await recordViaPostgres({ sessionId, idea, githubUrl, result }).catch((err) => ({
    ok: false,
    reason: err instanceof Error ? err.message : 'pg-failed',
  }))
  if (pg.ok) return pg

  return {
    ok: false,
    reason: `http:${http.reason || 'fail'}; pg:${pg.reason || 'fail'}`,
    sessionId,
  }
}
