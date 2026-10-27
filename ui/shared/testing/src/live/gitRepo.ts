/**
 * Throwaway git repos for the live-contract script tests, isolated from the developer's git config (a global
 * commit.gpgsign or hooksPath would otherwise make setup commits fail silently). Setup failures throw.
 */
import { spawnSync } from 'node:child_process'

const ENV = { ...process.env, GIT_CONFIG_GLOBAL: '/dev/null', GIT_CONFIG_NOSYSTEM: '1' }
const ISOLATE = [
  '-c',
  'commit.gpgsign=false',
  '-c',
  'core.hooksPath=/dev/null',
  '-c',
  'user.email=t@localhost',
  '-c',
  'user.name=t',
]

/** `git init` a new repo at `dir`. */
export function initRepo(dir: string): void {
  const r = spawnSync('git', ['init', '-q', dir], { encoding: 'utf8', env: ENV })
  if (r.status !== 0) throw new Error(`test repo setup failed: git init: ${r.stderr}`)
}

/** Run git in `dir`; a non-zero exit throws with git's stderr. */
export function gitIn(dir: string) {
  return (...args: string[]) => {
    const r = spawnSync('git', ['-C', dir, ...ISOLATE, ...args], { encoding: 'utf8', env: ENV })
    if (r.status !== 0)
      throw new Error(`test repo setup failed: git ${args.join(' ')}: ${r.stderr}`)
    return r
  }
}
