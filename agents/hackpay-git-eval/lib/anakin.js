/**
 * Anakin scrape client + deep GitHub repo surf (tree sample → file contents → report corpus).
 */
const ANAKIN_BASE = 'https://api.anakin.io/v1'

const MAX_FILES = Number(process.env.ANAKIN_DEEP_MAX_FILES || 24)
const MAX_FILE_BYTES = Number(process.env.ANAKIN_DEEP_MAX_FILE_BYTES || 48_000)
const MAX_TOTAL_CHARS = Number(process.env.ANAKIN_DEEP_MAX_TOTAL_CHARS || 120_000)

const PRIORITY_NAMES = new Set(
  [
    'readme.md',
    'readme',
    'package.json',
    'cargo.toml',
    'go.mod',
    'pyproject.toml',
    'requirements.txt',
    'dockerfile',
    'docker-compose.yml',
    'docker-compose.yaml',
    'makefile',
    'tsconfig.json',
    'next.config.js',
    'next.config.ts',
    'vite.config.ts',
    'vite.config.js',
    'app.py',
    'main.py',
    'main.ts',
    'main.js',
    'index.ts',
    'index.js',
    'server.js',
    'server.ts',
    'agentcard.json',
    'license',
    'license.md',
    'contributing.md',
    'changelog.md',
  ].map((s) => s.toLowerCase()),
)

const SKIP_DIR =
  /(?:^|\/)(?:node_modules|\.git|\.next|dist|build|coverage|vendor|__pycache__|\.turbo|\.cache|target)(?:\/|$)/i
const TEXT_EXT =
  /\.(md|mdx|txt|json|ya?ml|toml|js|jsx|ts|tsx|mjs|cjs|py|rs|go|java|kt|swift|rb|php|cs|cpp|c|h|hpp|css|scss|html|vue|svelte|sh|bash|zsh|sql|graphql|prisma|env\.example)$/i

function apiKey() {
  return process.env.ANAKIN_API_KEY?.trim() || ''
}

export function isAnakinConfigured() {
  return Boolean(apiKey())
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms))
}

function githubHeaders() {
  return {
    Accept: 'application/vnd.github+json',
    'User-Agent': 'hackpay-nasiko-git-eval-deep',
    ...(process.env.GITHUB_TOKEN?.trim()
      ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN.trim()}` }
      : {}),
  }
}

async function fetchGithubJson(path) {
  const res = await fetch(`https://api.github.com${path}`, { headers: githubHeaders() })
  if (!res.ok) throw new Error(`GitHub ${res.status} for ${path}`)
  return res.json()
}

export async function anakinScrapeUrl(url, opts = {}) {
  const key = apiKey()
  if (!key) return { ok: false, error: 'ANAKIN_API_KEY is not set' }

  try {
    const start = await fetch(`${ANAKIN_BASE}/url-scraper`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json', 'X-API-Key': key },
      body: JSON.stringify({ url, useBrowser: Boolean(opts.useBrowser) }),
    })
    if (!start.ok) {
      const text = await start.text().catch(() => '')
      return { ok: false, error: text.slice(0, 200) || `Anakin ${start.status}` }
    }
    const { jobId, error } = await start.json()
    if (!jobId) return { ok: false, error: error || 'no jobId' }

    const deadline = Date.now() + (opts.timeoutMs || 90_000)
    while (Date.now() < deadline) {
      await sleep(3000)
      const poll = await fetch(`${ANAKIN_BASE}/url-scraper/${encodeURIComponent(jobId)}`, {
        headers: { 'X-API-Key': key },
      })
      const body = await poll.json()
      if (body.status === 'completed') return { ok: true, markdown: body.markdown || '' }
      if (body.status === 'failed') return { ok: false, error: body.error || 'failed' }
    }
    return { ok: false, error: 'timeout' }
  } catch (err) {
    return { ok: false, error: err instanceof Error ? err.message : 'anakin error' }
  }
}

function filePriority(path) {
  const base = path.split('/').pop()?.toLowerCase() || ''
  const lower = path.toLowerCase()
  let score = 0
  if (PRIORITY_NAMES.has(base)) score += 100
  if (/^readme/i.test(base)) score += 80
  if (/(^|\/)(src|app|lib|server|api|agents)\//i.test(lower)) score += 40
  if (/(^|\/)(test|tests|__tests__|spec)\//i.test(lower)) score += 25
  if (/\.(ts|tsx|js|jsx|py|rs|go)$/i.test(base)) score += 20
  if (/\.(md|json|ya?ml|toml)$/i.test(base)) score += 15
  if (/^dockerfile$/i.test(base)) score += 50
  score -= Math.min(30, path.split('/').length * 3)
  return score
}

function selectDeepPaths(treePaths) {
  const candidates = treePaths
    .filter((p) => p && !SKIP_DIR.test(p))
    .filter((p) => {
      const base = p.split('/').pop() || ''
      return (
        TEXT_EXT.test(base) ||
        PRIORITY_NAMES.has(base.toLowerCase()) ||
        /^readme/i.test(base) ||
        /^dockerfile$/i.test(base)
      )
    })
    .map((path) => ({ path, score: filePriority(path) }))
    .sort((a, b) => b.score - a.score)

  const picked = []
  const seen = new Set()
  for (const c of candidates) {
    if (seen.has(c.path)) continue
    seen.add(c.path)
    picked.push(c.path)
    if (picked.length >= MAX_FILES) break
  }
  return picked
}

async function fetchRawFile(owner, repo, branch, filePath) {
  const url = `https://raw.githubusercontent.com/${owner}/${repo}/${encodeURIComponent(branch)}/${filePath
    .split('/')
    .map(encodeURIComponent)
    .join('/')}`
  const res = await fetch(url, {
    headers: {
      'User-Agent': 'hackpay-nasiko-git-eval-deep',
      ...(process.env.GITHUB_TOKEN?.trim() ? githubHeaders() : {}),
    },
  })
  if (!res.ok) return { ok: false, error: `raw ${res.status}`, text: '' }
  const buf = Buffer.from(await res.arrayBuffer())
  if (buf.includes(0)) return { ok: false, error: 'binary', text: '' }
  if (buf.length > MAX_FILE_BYTES) {
    return { ok: true, text: buf.subarray(0, MAX_FILE_BYTES).toString('utf8') + '\n…[truncated]' }
  }
  return { ok: true, text: buf.toString('utf8') }
}

export async function anakinSurfGithubRepo(githubUrl) {
  const base = githubUrl.replace(/\/$/, '')
  const errors = []
  let repoMarkdown = ''
  let readmeMarkdown = ''

  const home = await anakinScrapeUrl(base)
  if (home.ok) repoMarkdown = home.markdown || ''
  else errors.push(home.error || 'repo')

  for (const path of ['/blob/main/README.md', '/blob/master/README.md']) {
    const page = await anakinScrapeUrl(`${base}${path}`)
    if (page.ok && (page.markdown || '').length > 80) {
      readmeMarkdown = page.markdown || ''
      break
    }
    if (!page.ok) errors.push(`${path}: ${page.error}`)
  }

  return { ok: Boolean(repoMarkdown || readmeMarkdown), repoMarkdown, readmeMarkdown, errors }
}

export async function anakinDeepSurfGithubRepo(githubUrl) {
  const match = String(githubUrl || '').match(/github\.com\/([^/\s]+)\/([^/\s?#]+)/i)
  if (!match) {
    return {
      ok: false,
      files: [],
      repoMarkdown: '',
      readmeMarkdown: '',
      corpus: '',
      errors: ['invalid github url'],
      stats: { treeFiles: 0, reviewed: 0, chars: 0, branch: '' },
    }
  }
  const owner = match[1]
  const repo = match[2].replace(/\.git$/i, '')
  const base = `https://github.com/${owner}/${repo}`
  const errors = []
  /** @type {Array<{ path: string, chars: number, source: string, text: string }>} */
  const files = []

  let repoMarkdown = ''
  if (isAnakinConfigured()) {
    const home = await anakinScrapeUrl(base, { useBrowser: false, timeoutMs: 60_000 })
    if (home.ok) repoMarkdown = home.markdown || ''
    else errors.push(`anakin home: ${home.error}`)
  }

  let branch = 'main'
  let treePaths = []
  try {
    const meta = await fetchGithubJson(`/repos/${owner}/${repo}`)
    branch = meta.default_branch || 'main'
    const tree = await fetchGithubJson(
      `/repos/${owner}/${repo}/git/trees/${encodeURIComponent(branch)}?recursive=1`,
    )
    treePaths = (tree.tree || [])
      .filter((n) => n.type === 'blob' && typeof n.path === 'string')
      .map((n) => n.path)
  } catch (err) {
    errors.push(err instanceof Error ? err.message : 'tree failed')
    const shallow = await anakinSurfGithubRepo(base)
    return {
      ok: shallow.ok,
      files: [],
      repoMarkdown: shallow.repoMarkdown || repoMarkdown,
      readmeMarkdown: shallow.readmeMarkdown,
      corpus: [shallow.repoMarkdown, shallow.readmeMarkdown].filter(Boolean).join('\n\n'),
      errors: [...errors, ...shallow.errors],
      stats: {
        treeFiles: 0,
        reviewed: 0,
        chars: (shallow.readmeMarkdown || '').length,
        branch,
      },
    }
  }

  const selected = selectDeepPaths(treePaths)
  let totalChars = 0
  let readmeMarkdown = ''

  for (const filePath of selected) {
    if (totalChars >= MAX_TOTAL_CHARS) break
    let text = ''
    let source = 'raw'
    const fetched = await fetchRawFile(owner, repo, branch, filePath)
    if (fetched.ok && fetched.text?.trim()) {
      text = fetched.text
    } else if (files.length < 5 && isAnakinConfigured()) {
      const scraped = await anakinScrapeUrl(`${base}/blob/${branch}/${filePath}`, {
        timeoutMs: 45_000,
      })
      if (scraped.ok && scraped.markdown) {
        text = scraped.markdown.slice(0, MAX_FILE_BYTES)
        source = 'anakin'
      } else {
        errors.push(`${filePath}: ${fetched.error || scraped.error}`)
        continue
      }
    } else {
      errors.push(`${filePath}: ${fetched.error || 'empty'}`)
      continue
    }

    files.push({ path: filePath, chars: text.length, source, text })
    totalChars += text.length
    const baseName = filePath.split('/').pop() || ''
    if (/^readme/i.test(baseName) && text.length > readmeMarkdown.length) readmeMarkdown = text
  }

  const parts = []
  if (repoMarkdown) {
    parts.push(`## Repository overview (Anakin)\n\n${repoMarkdown.slice(0, 12_000)}`)
  }
  let used = parts.join('\n').length
  for (const f of files) {
    if (used >= MAX_TOTAL_CHARS) break
    const chunk = `## File: ${f.path}\n\n${f.text}`
    const room = MAX_TOTAL_CHARS - used
    parts.push(chunk.slice(0, room))
    used += Math.min(chunk.length, room)
  }

  const corpus = parts.join('\n\n---\n\n')
  return {
    ok: files.length > 0 || Boolean(repoMarkdown || readmeMarkdown),
    files: files.map(({ path, chars, source }) => ({ path, chars, source })),
    repoMarkdown,
    readmeMarkdown,
    corpus,
    errors: errors.slice(0, 12),
    stats: {
      treeFiles: treePaths.length,
      reviewed: files.length,
      chars: corpus.length,
      branch,
    },
  }
}

export function buildDeepNarrative({ idea, owner, repo, deep }) {
  const paths = (deep.files || []).map((f) => f.path)
  const hasTests = paths.some((p) => /(^|\/)(test|tests|__tests__|spec)(\/|\.|$)/i.test(p))
  const hasCi = paths.some((p) => /\.github\/workflows\//i.test(p) || /ci\./i.test(p))
  const hasDocker = paths.some((p) => /dockerfile|docker-compose/i.test(p))
  const hasApi = paths.some((p) => /(^|\/)(api|routes|server|handlers)\//i.test(p))
  const hasFrontend = paths.some(
    (p) => /\.(tsx|jsx|vue|svelte)$/i.test(p) || /(^|\/)(app|pages|components)\//i.test(p),
  )
  const hasAgents = paths.some((p) => /(^|\/)agents\//i.test(p) || /agentcard/i.test(p))
  const langs = new Set()
  for (const p of paths) {
    const m = p.match(/\.([a-z0-9]+)$/i)
    if (m) langs.add(m[1].toLowerCase())
  }

  const ideaTokens = String(idea || '')
    .toLowerCase()
    .replace(/[^a-z0-9\s]/g, ' ')
    .split(/\s+/)
    .filter((w) => w.length > 3)
  const corpusLower = String(deep.corpus || '').toLowerCase()
  const ideaHits = ideaTokens.filter((t) => corpusLower.includes(t))
  const ideaCoverage = ideaTokens.length
    ? Math.round((ideaHits.length / ideaTokens.length) * 100)
    : 0

  const lines = []
  lines.push(`# Deep repository report — ${owner}/${repo}`)
  lines.push('')
  lines.push(
    `Anakin + GitHub tree analysis reviewed **${deep.stats?.reviewed || 0}** text files` +
      ` (of **${deep.stats?.treeFiles || 0}** in \`${deep.stats?.branch || 'main'}\`),` +
      ` ~${Math.round((deep.stats?.chars || 0) / 1000)}k characters of source/docs.`,
  )
  lines.push('')
  lines.push('## Idea alignment')
  lines.push(
    ideaTokens.length
      ? `About **${ideaCoverage}%** of distinctive idea keywords appear across the sampled files` +
          (ideaHits.length ? ` (e.g. ${ideaHits.slice(0, 8).join(', ')})` : '') +
          '.'
      : 'No detailed idea text was provided; alignment scored from repository structure only.',
  )
  lines.push('')
  lines.push('## Architecture signals')
  const arch = []
  if (hasFrontend) arch.push('frontend / UI')
  if (hasApi) arch.push('API / server')
  if (hasAgents) arch.push('agent / automation')
  if (hasDocker) arch.push('containerization')
  if (hasCi) arch.push('CI workflows')
  if (hasTests) arch.push('automated tests')
  lines.push(arch.length ? `Detected: ${arch.join(', ')}.` : 'Limited structural signals in the sampled set.')
  if (langs.size) lines.push(`Extensions seen: ${[...langs].slice(0, 12).join(', ')}.`)
  lines.push('')
  lines.push('## Files reviewed')
  for (const f of (deep.files || []).slice(0, 30)) {
    lines.push(`- \`${f.path}\` (${f.chars} chars, via ${f.source})`)
  }
  if (!(deep.files || []).length) lines.push('- (tree unavailable — Anakin overview / README only)')
  lines.push('')
  lines.push('## Findings')
  if (hasTests) lines.push('- Test paths present — good signal for maintainability.')
  else lines.push('- Little/no test layout in the sample — consider adding tests.')
  if (hasCi) lines.push('- CI config found under `.github/workflows` or similar.')
  else lines.push('- No CI workflow spotted in the sample.')
  if ((deep.readmeMarkdown || '').length > 200) lines.push('- README has meaningful depth.')
  else lines.push('- README is thin or missing in the sample — expand setup + demo.')
  if (hasAgents) lines.push('- Agent-related packaging detected (fits Nasiko / A2A style submissions).')
  lines.push('')
  lines.push('## Overall')
  lines.push(
    `This is an automated deep pass (capped at ${MAX_FILES} files). ` +
      'Advisory for hackathon judging — humans still confirm winners and dual-control payouts.',
  )

  return {
    markdown: lines.join('\n'),
    ideaCoverage,
    flags: { hasTests, hasCi, hasDocker, hasApi, hasFrontend, hasAgents },
  }
}
