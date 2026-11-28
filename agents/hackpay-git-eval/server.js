/**
 * HackPay Git Eval agent — Nasiko-native (port 8000) + HackPay /evaluate.
 *
 * - GET  /health
 * - GET  /.well-known/agent.json | agent-card.json
 * - POST /evaluate  — HackPay webhook body
 * - POST /          — A2A JSON-RPC (message/send) for Nasiko chat/proxy
 */
import http from 'node:http'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const __dirname = path.dirname(fileURLToPath(import.meta.url))

function loadEnvFile(filePath, { override = false, skipKeys = [] } = {}) {
  if (!fs.existsSync(filePath)) return false
  const skip = new Set(skipKeys)
  const text = fs.readFileSync(filePath, 'utf8')
  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim()
    if (!trimmed || trimmed.startsWith('#')) continue
    const eq = trimmed.indexOf('=')
    if (eq <= 0) continue
    const key = trimmed.slice(0, eq).trim()
    if (!key || skip.has(key)) continue
    let value = trimmed.slice(eq + 1).trim()
    if (
      (value.startsWith('"') && value.endsWith('"')) ||
      (value.startsWith("'") && value.endsWith("'"))
    ) {
      value = value.slice(1, -1)
    }
    if (!override && process.env[key] !== undefined && process.env[key] !== '') continue
    process.env[key] = value
  }
  return true
}

loadEnvFile(path.join(__dirname, '.env'), { override: true })
// When copied under nasiko/agents, HackPay .env may be elsewhere — optional
loadEnvFile(path.join(__dirname, '../../../HackPay/.env'), { override: false, skipKeys: ['PORT'] })
loadEnvFile(path.join(__dirname, '../../.env'), { override: false, skipKeys: ['PORT'] })

const { evaluateSubmission } = await import('./lib/evaluate.js')
const { recordEvaluateSession } = await import('./lib/sessionHistory.js')

// Nasiko convention: agents listen on 8000 inside the container
const PORT = Number(process.env.PORT || 8000)

function readJson(req) {
  return new Promise((resolve, reject) => {
    const chunks = []
    req.on('data', (c) => chunks.push(c))
    req.on('end', () => {
      try {
        resolve(JSON.parse(Buffer.concat(chunks).toString('utf8') || '{}'))
      } catch (err) {
        reject(err)
      }
    })
    req.on('error', reject)
  })
}

function send(res, status, body) {
  res.writeHead(status, {
    'Content-Type': 'application/json',
    'Access-Control-Allow-Origin': '*',
    'A2A-Version': '1.0',
  })
  res.end(JSON.stringify(body))
}

function extractTextFromA2A(params) {
  const msg = params?.message || params?.params?.message || params
  const parts = msg?.parts || []
  const texts = []
  for (const p of parts) {
    if (typeof p?.text === 'string') texts.push(p.text)
    if (typeof p?.root?.text === 'string') texts.push(p.root.text)
  }
  if (!texts.length && typeof msg?.text === 'string') texts.push(msg.text)
  return texts.join('\n').trim()
}

function parseEvalPayload(raw) {
  if (raw && typeof raw === 'object' && (raw.idea || raw.githubUrl || raw.github_url)) {
    return raw
  }
  const text = typeof raw === 'string' ? raw : ''
  try {
    const jsonStart = text.indexOf('{')
    if (jsonStart >= 0) {
      return JSON.parse(text.slice(jsonStart))
    }
  } catch {
    /* fall through */
  }
  const urlMatch = text.match(/https?:\/\/github\.com\/[^\s]+/i)
  return {
    idea: text.slice(0, 500) || 'Hackathon submission',
    githubUrl: urlMatch ? urlMatch[0] : '',
  }
}

async function postAssessment(callbackUrl, payload) {
  if (!callbackUrl) return { ok: false, skipped: true, error: 'no callbackUrl' }
  const secret =
    process.env.HACKPAY_CALLBACK_SECRET || process.env.SUBMISSION_AGENT_CALLBACK_SECRET || ''
  const res = await fetch(callbackUrl, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      ...(secret ? { 'x-hackpay-agent-secret': secret, 'api-key': secret } : {}),
    },
    body: JSON.stringify(payload),
  })
  if (!res.ok) {
    const text = await res.text().catch(() => '')
    return { ok: false, skipped: false, error: text.slice(0, 200) || res.statusText }
  }
  return { ok: true, skipped: false }
}

async function runEvaluate(body) {
  const idea = String(body.idea || '')
  const githubUrl = String(body.githubUrl || body.github_url || '')
  const hackathonId = String(body.hackathonId || body.hackathon_id || '')
  const walletAddress = String(body.walletAddress || body.wallet_address || '').toLowerCase()
  const callbackUrl = String(body.callbackUrl || process.env.HACKPAY_CALLBACK_URL || '')

  if (!idea || !githubUrl) {
    throw new Error('idea and githubUrl are required')
  }

  const assessment = await evaluateSubmission({ idea, githubUrl })
  let callback = { ok: true, skipped: true }
  if (callbackUrl && hackathonId && walletAddress) {
    callback = await postAssessment(callbackUrl, { hackathonId, walletAddress, assessment })
  }

  const result = {
    ok: true,
    source: 'nasiko-git-eval',
    surf: 'anakin',
    hackathonId,
    walletAddress,
    assessment,
    callback,
  }

  // Best-effort: surface this run on Nasiko sessions.html Execution history.
  const history = await recordEvaluateSession({ idea, githubUrl, result }).catch((err) => ({
    ok: false,
    reason: err instanceof Error ? err.message : 'history failed',
  }))
  if (!history.ok) {
    console.warn('[hackpay-git-eval] session history skipped:', history.reason || 'unknown')
  } else {
    console.log(`[hackpay-git-eval] session history via ${history.via}: ${history.sessionId}`)
  }
  result.sessionId = history.sessionId || undefined

  return result
}

const server = http.createServer(async (req, res) => {
  const url = new URL(req.url || '/', `http://${req.headers.host}`)

  if (req.method === 'OPTIONS') {
    res.writeHead(204, {
      'Access-Control-Allow-Origin': '*',
      'Access-Control-Allow-Methods': 'GET,POST,OPTIONS',
      'Access-Control-Allow-Headers':
        'Content-Type, Authorization, api-key, x-hackpay-agent-secret, A2A-Version',
    })
    res.end()
    return
  }

  if (req.method === 'GET' && (url.pathname === '/health' || url.pathname === '/ready')) {
    send(res, 200, {
      ok: true,
      service: 'hackpay-git-eval',
      anakin: Boolean(process.env.ANAKIN_API_KEY?.trim()),
      port: PORT,
    })
    return
  }

  if (
    req.method === 'GET' &&
    (url.pathname === '/.well-known/agent.json' ||
      url.pathname === '/.well-known/agent-card.json' ||
      url.pathname === '/agent-card')
  ) {
    const card = fs.readFileSync(path.join(__dirname, 'AgentCard.json'), 'utf8')
    res.writeHead(200, { 'Content-Type': 'application/json', 'A2A-Version': '1.0' })
    res.end(card)
    return
  }

  if (req.method === 'GET' && url.pathname === '/') {
    send(res, 200, {
      ok: true,
      service: 'hackpay-git-eval',
      endpoints: ['/health', '/evaluate', '/.well-known/agent.json', 'POST / (A2A JSON-RPC)'],
    })
    return
  }

  if (req.method === 'POST' && url.pathname === '/evaluate') {
    try {
      const body = await readJson(req)
      const result = await runEvaluate(body)
      send(res, 200, result)
    } catch (err) {
      send(res, 500, { ok: false, error: err instanceof Error ? err.message : 'evaluate failed' })
    }
    return
  }

  // A2A JSON-RPC (Nasiko proxy / nasiko chat)
  if (req.method === 'POST' && (url.pathname === '/' || url.pathname === '/a2a')) {
    try {
      const rpc = await readJson(req)
      const id = rpc.id ?? null
      const method = rpc.method || ''

      if (method === 'agent/authenticatedExtendedCard' || method === 'agent/getAuthenticatedExtendedCard') {
        const card = JSON.parse(fs.readFileSync(path.join(__dirname, 'AgentCard.json'), 'utf8'))
        send(res, 200, { jsonrpc: '2.0', id, result: card })
        return
      }

      if (method === 'message/send' || method === 'tasks/send' || method === 'SendMessage') {
        const text = extractTextFromA2A(rpc.params || {})
        const payload = parseEvalPayload(text)
        const result = await runEvaluate(payload)
        const reply = JSON.stringify(result, null, 2)
        send(res, 200, {
          jsonrpc: '2.0',
          id,
          result: {
            id: `task_${Date.now()}`,
            contextId: rpc.params?.message?.contextId || `ctx_${Date.now()}`,
            status: { state: 'completed' },
            history: [
              {
                role: 'agent',
                parts: [{ type: 'text', text: reply }],
              },
            ],
            artifacts: [
              {
                name: 'assessment',
                parts: [{ type: 'text', text: reply }],
              },
            ],
          },
        })
        return
      }

      // Direct HackPay JSON posted to /
      if (rpc.idea || rpc.githubUrl || rpc.event === 'repo_submission.saved') {
        const result = await runEvaluate(rpc)
        send(res, 200, result)
        return
      }

      send(res, 200, {
        jsonrpc: '2.0',
        id,
        error: { code: -32601, message: `Method not found: ${method || '(empty)'}` },
      })
    } catch (err) {
      send(res, 200, {
        jsonrpc: '2.0',
        id: null,
        error: {
          code: -32000,
          message: err instanceof Error ? err.message : 'agent error',
        },
      })
    }
    return
  }

  send(res, 404, { ok: false, error: 'not found' })
})

server.listen(PORT, '0.0.0.0', () => {
  console.log(`[hackpay-git-eval] listening on 0.0.0.0:${PORT}`)
  console.log(`[hackpay-git-eval] Anakin: ${process.env.ANAKIN_API_KEY ? 'configured' : 'MISSING'}`)
})
