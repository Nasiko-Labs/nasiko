// @vitest-environment node
/** The npm test staleness warning (scripts/lib/staleness.ts, plans/feat-live-contract.md §7.6) against a throwaway git repo. */
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { gitIn, initRepo } from './gitRepo'
import { fixtureShas, stalenessWarning } from '../../../../scripts/lib/staleness.ts'

let dir = ''
const repo = () => join(dir, 'cloud-rs')
const live = () => join(dir, '__live__')
const git = (...args: string[]) => gitIn(repo())(...args)
const commit = (file: string, text: string) => {
  mkdirSync(join(repo(), file, '..'), { recursive: true })
  writeFileSync(join(repo(), file), text)
  git('add', '.')
  git('commit', '-qm', file)
  return git('rev-parse', 'HEAD').stdout.trim()
}
const fixture = (edition: 'oss' | 'ee', id: string, sha: string) => {
  mkdirSync(join(live(), edition, 'f'), { recursive: true })
  writeFileSync(join(live(), edition, 'f', `${id}.json`), JSON.stringify({ id, server: { sha } }))
}

beforeEach(() => {
  dir = mkdtempSync(join(tmpdir(), 'staleness-'))
  initRepo(repo())
})
afterEach(() => rmSync(dir, { recursive: true, force: true }))

describe('stalenessWarning', () => {
  it('is silent while the server code is unchanged since the recorded sha (other paths may change)', () => {
    const sha = commit('oss/server.rs', 'v1')
    commit('ui/app.ts', 'ui change')
    fixture('oss', 'a', sha)
    expect(stalenessWarning(live(), repo())).toBeNull()
  })
  it('names the recorded sha, its editions, HEAD and the re-record command once the server changed', () => {
    const sha = commit('oss/server.rs', 'v1')
    const head = commit('ee/server.rs', 'v2')
    fixture('oss', 'a', sha)
    fixture('ee', 'b', sha)
    const w = stalenessWarning(live(), repo())!
    expect(w).toContain(`fixtures at ${sha.slice(0, 8)} (oss, ee)`)
    expect(w).toContain(`checkout at ${head.slice(0, 8)}`)
    expect(w).toContain('npm run record:live')
  })
  it('is silent without a checkout, with an unknown sha, or with no fixtures (CI)', () => {
    fixture('oss', 'a', 'deadbeefdeadbeefdeadbeefdeadbeefdeadbeef')
    expect(stalenessWarning(live(), join(dir, 'missing'))).toBeNull()
    commit('oss/server.rs', 'v1')
    expect(stalenessWarning(live(), repo())).toBeNull()
    expect(stalenessWarning(join(dir, 'none'), repo())).toBeNull()
  })
})

describe('fixtureShas', () => {
  it('collects distinct shas per edition and ignores fixtures without one', () => {
    fixture('oss', 'a', 'abcdef1234567')
    fixture('oss', 'b', 'abcdef1234567')
    fixture('ee', 'c', 'abcdef1234567')
    mkdirSync(join(live(), 'oss', 'f'), { recursive: true })
    writeFileSync(
      join(live(), 'oss', 'f', 'hand.json'),
      JSON.stringify({ id: 'hand', source: 'hand-written' }),
    )
    expect([...fixtureShas(live())].map(([s, e]) => [s, [...e]])).toEqual([
      ['abcdef1234567', ['oss', 'ee']],
    ])
  })
})
