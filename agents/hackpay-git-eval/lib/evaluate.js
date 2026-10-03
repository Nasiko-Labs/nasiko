/**
 * Scoring + deep Anakin/GitHub tree analysis for HackPay submissions.
 */
import {
  anakinDeepSurfGithubRepo,
  buildDeepNarrative,
  isAnakinConfigured,
} from './anakin.js'

function clamp(n) {
  return Math.max(0, Math.min(100, Math.round(n)))
}

function tokenize(text) {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9\s]/g, ' ')
    .split(/\s+/)
    .filter((w) => w.length > 2)
}

function parseGithubRepo(value) {
  const text = String(value || '').trim()
  const match = text.match(/github\.com\/([^/\s]+)\/([^/\s?#]+)/i)
  if (match) {
    const owner = match[1]
    const repo = match[2].replace(/\.git$/i, '')
    return { owner, repo, url: `https://github.com/${owner}/${repo}` }
  }
  if (/^[^/\s]+\/[^/\s]+$/.test(text)) {
    const [owner, repoRaw] = text.split('/')
    const repo = repoRaw.replace(/\.git$/i, '')
    return { owner, repo, url: `https://github.com/${owner}/${repo}` }
  }
  return null
}

async function fetchGithubJson(path) {
  const res = await fetch(`https://api.github.com${path}`, {
    headers: {
      Accept: 'application/vnd.github+json',
      'User-Agent': 'hackpay-nasiko-git-eval',
      ...(process.env.GITHUB_TOKEN?.trim()
        ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN.trim()}` }
        : {}),
    },
  })
  if (!res.ok) throw new Error(`GitHub ${res.status} for ${path}`)
  return res.json()
}

async function fetchReadmeText(owner, repo) {
  try {
    const data = await fetchGithubJson(`/repos/${owner}/${repo}/readme`)
    if (!data.content) return ''
    if (data.encoding === 'base64') return Buffer.from(data.content, 'base64').toString('utf8')
    return String(data.content)
  } catch {
    return ''
  }
}

function ideaFitScore(idea, meta, corpusText, ideaCoverage) {
  if (typeof ideaCoverage === 'number' && ideaCoverage > 0) {
    return clamp(25 + ideaCoverage * 0.7)
  }
  const ideaTokens = new Set(tokenize(idea))
  if (!ideaTokens.size) return 40
  const corpus = tokenize(
    [meta.description || '', meta.full_name || '', ...(meta.topics || []), corpusText.slice(0, 8000)].join(
      ' ',
    ),
  )
  if (!corpus.length) return 35
  const corpusSet = new Set(corpus)
  let hits = 0
  for (const t of ideaTokens) if (corpusSet.has(t)) hits += 1
  return clamp(30 + (hits / ideaTokens.size) * 70)
}

function buildScores({ ideaFit, meta, hasReadme, readmeLen, flags, filesReviewed }) {
  const licenseOk = Boolean(meta.license?.spdx_id && meta.license.spdx_id !== 'NOASSERTION')
  const recent =
    meta.pushed_at && Date.now() - new Date(meta.pushed_at).getTime() < 1000 * 60 * 60 * 24 * 180

  let completeness = 30
  if (hasReadme) completeness += 20
  if (readmeLen > 400) completeness += 10
  if (licenseOk) completeness += 10
  if (meta.language) completeness += 8
  if (filesReviewed >= 8) completeness += 12
  if (flags?.hasApi || flags?.hasFrontend) completeness += 8
  if (flags?.hasDocker) completeness += 5

  let activity = 30
  if (recent) activity += 35
  if ((meta.stargazers_count || 0) > 0) activity += 15
  if ((meta.forks_count || 0) > 0) activity += 10
  if (!meta.archived) activity += 10

  let docs = 20
  if (hasReadme) docs += 25
  if (readmeLen > 800) docs += 15
  if (meta.homepage) docs += 8
  if ((meta.topics || []).length) docs += 10
  if (flags?.hasCi) docs += 10
  if (flags?.hasTests) docs += 12

  const ideaFitN = clamp(ideaFit)
  const completenessN = clamp(completeness)
  const activityN = clamp(activity)
  const docsN = clamp(docs)
  return {
    ideaFit: ideaFitN,
    completeness: completenessN,
    activity: activityN,
    docs: docsN,
    overall: clamp(ideaFitN * 0.35 + completenessN * 0.25 + activityN * 0.2 + docsN * 0.2),
  }
}

export async function evaluateSubmission({ idea, githubUrl }) {
  const parsed = parseGithubRepo(githubUrl)
  if (!parsed) throw new Error('Invalid GitHub repository URL')

  let meta = {
    full_name: `${parsed.owner}/${parsed.repo}`,
    description: null,
    topics: [],
    license: null,
    language: null,
    stargazers_count: 0,
    forks_count: 0,
    pushed_at: null,
    archived: false,
    homepage: null,
  }
  let readme = ''
  let usedAnakin = false
  let anakinNote = ''
  let deepReport = ''
  let filesReviewed = []
  let flags = {}
  let ideaCoverage = 0
  let corpus = ''

  const deepEnabled = (process.env.ANAKIN_DEEP_SURF || 'true').toLowerCase() !== 'false'

  if (isAnakinConfigured() && deepEnabled) {
    usedAnakin = true
    const deep = await anakinDeepSurfGithubRepo(parsed.url)
    corpus = deep.corpus || ''
    readme = deep.readmeMarkdown || deep.repoMarkdown || ''
    filesReviewed = deep.files || []
    const narrative = buildDeepNarrative({
      idea,
      owner: parsed.owner,
      repo: parsed.repo,
      deep,
    })
    deepReport = narrative.markdown
    flags = narrative.flags || {}
    ideaCoverage = narrative.ideaCoverage || 0
    anakinNote = deep.ok
      ? ` Deep Anakin/GitHub pass: ${deep.stats?.reviewed || 0}/${deep.stats?.treeFiles || 0} files on \`${deep.stats?.branch || 'main'}\`.`
      : ` Deep surf partial: ${(deep.errors || []).slice(0, 2).join('; ')}.`
  } else if (isAnakinConfigured()) {
    usedAnakin = true
    anakinNote = ' Shallow Anakin mode (set ANAKIN_DEEP_SURF=true for full tree sample).'
  }

  try {
    meta = await fetchGithubJson(`/repos/${parsed.owner}/${parsed.repo}`)
    const apiReadme = await fetchReadmeText(parsed.owner, parsed.repo)
    if (apiReadme && apiReadme.length > readme.length) readme = apiReadme
  } catch (err) {
    const msg = err instanceof Error ? err.message : 'GitHub fetch failed'
    anakinNote += ` GitHub API skipped (${msg}).`
    if (!readme) {
      readme = `# ${parsed.owner}/${parsed.repo}\n\nPublic repo URL: ${parsed.url}\n`
    }
  }

  const scoringCorpus = corpus || readme
  const hasReadme = readme.trim().length > 40
  const scores = buildScores({
    ideaFit: ideaFitScore(idea, meta, scoringCorpus, ideaCoverage),
    meta,
    hasReadme,
    readmeLen: readme.length,
    flags,
    filesReviewed: filesReviewed.length,
  })

  const license = meta.license?.spdx_id || null
  const strengths = []
  const improvements = []
  if (scores.ideaFit >= 70) strengths.push('Idea–repository alignment across sampled files')
  else improvements.push('Tighten README/code so they mirror the stated idea')
  if (hasReadme && readme.length > 400) strengths.push('Documentation / README depth')
  else improvements.push('Expand README with problem, setup, and demo')
  if (license && license !== 'NOASSERTION') strengths.push(`Clear license (${license})`)
  else improvements.push('Add an SPDX license')
  if (meta.language) strengths.push(`${meta.language} implementation present`)
  if (flags.hasTests) strengths.push('Test layout detected in deep sample')
  else improvements.push('Add automated tests')
  if (flags.hasCi) strengths.push('CI workflow present')
  else improvements.push('Add CI (e.g. GitHub Actions)')
  if (filesReviewed.length >= 8) strengths.push(`Deep review of ${filesReviewed.length} source/docs files`)
  if (usedAnakin) strengths.push('Anakin deep repository surf')
  while (strengths.length < 3) {
    const pad = 'Public repository is reachable'
    if (strengths.includes(pad)) break
    strengths.push(pad)
  }
  const improvementPads = [
    'Add architecture notes or demo assets',
    'Add tests, demo link, or architecture notes',
    'Document setup and run instructions',
  ]
  for (const pad of improvementPads) {
    if (improvements.length >= 3) break
    if (!improvements.includes(pad)) improvements.push(pad)
  }

  return {
    assessedAt: new Date().toISOString(),
    provider: usedAnakin ? 'nasiko-anakin' : 'hackpay-heuristic',
    scores,
    evidence: {
      license,
      language: meta.language || null,
      stars: meta.stargazers_count || 0,
      lastPush: meta.pushed_at || null,
      description: meta.description || null,
      topics: meta.topics || [],
      hasReadme,
      filesReviewed: filesReviewed.length,
      treeSample: filesReviewed.slice(0, 24).map((f) => f.path),
      ideaCoverage,
      deepFlags: flags,
    },
    signals: [
      { label: 'License', value: license || 'None' },
      { label: 'Language', value: meta.language || 'Unknown' },
      { label: 'Stars', value: String(meta.stargazers_count || 0) },
      {
        label: 'Last push',
        value: meta.pushed_at ? new Date(meta.pushed_at).toLocaleDateString() : 'Unknown',
      },
      { label: 'README', value: hasReadme ? 'Yes' : 'Missing' },
      { label: 'Files reviewed', value: String(filesReviewed.length) },
      { label: 'Idea coverage', value: ideaCoverage ? `${ideaCoverage}%` : 'n/a' },
      { label: 'Anakin', value: usedAnakin ? 'Deep' : 'Off' },
    ],
    strengths: strengths.slice(0, 8),
    improvements: improvements.slice(0, 8),
    overallFeedback: `Overall ${scores.overall}/100. Idea fit ${scores.ideaFit}, completeness ${scores.completeness}, activity ${scores.activity}, docs ${scores.docs}.${anakinNote}`,
    aiFeedback: deepReport
      ? deepReport
      : `Nasiko git-eval assessed ${parsed.owner}/${parsed.repo}.${anakinNote} Advisory only — humans still approve payouts.`,
    deepReport: deepReport || undefined,
    filesReviewed,
  }
}
