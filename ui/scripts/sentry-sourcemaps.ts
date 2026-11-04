// Upload a release build's source maps to Sentry, then delete them, so stack traces are readable in Sentry while no
// map is ever embedded in a binary or served from /assets (a map would publish private source).
//
//   NASIKO_SOURCEMAPS=hidden node scripts/app.ts build [edition]
//   SENTRY_AUTH_TOKEN=… SENTRY_ORG=nasiko-labs SENTRY_PROJECT=nasiko-ui node scripts/sentry-sourcemaps.ts [edition…]
//   node scripts/sentry-sourcemaps.ts --delete-only [edition…]      (no upload: just make the dist safe to embed)
//
// Per edition: sentry-cli injects a debug ID into each script and its map (Sentry pairs them by that ID, so no file
// naming or release matching can go wrong), the changed scripts are compressed again (the servers serve the .br/.gz
// variants, which must carry the same ID), the maps are uploaded under the build's release name
// (dist/.vite/sentry-release.txt), and every map is deleted. The delete runs even when an earlier step fails.
// The auth token is read from the environment only (CI's secret store): never pass it as an argument or write it to a
// file in the repository (no .sentryclirc, no .env.sentry-build-plugin).
import { execFileSync } from 'node:child_process'
import { existsSync, readdirSync, readFileSync, rmSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { findEditions } from './editions.ts'
import { precompress } from './lib/precompress.ts'

/** Pinned: this runs with CI's credentials. */
const SENTRY_CLI = '@sentry/cli@3.8.0'
const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')

function walk(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? walk(join(dir, e.name)) : [join(dir, e.name)],
  )
}

const mapsIn = (dist: string) => walk(dist).filter((f) => /\.map(\.br|\.gz)?$/.test(f))

function sentryCli(args: string[]) {
  execFileSync('npx', ['--yes', SENTRY_CLI, ...args], { stdio: 'inherit', env: process.env })
}

function upload(dist: string) {
  const assets = join(dist, 'assets')
  const release = readFileSync(join(dist, '.vite/sentry-release.txt'), 'utf8').trim()
  sentryCli(['sourcemaps', 'inject', assets])
  const scripts = walk(assets).filter((f) => /\.m?js$/.test(f))
  for (const file of scripts) for (const ext of ['.br', '.gz']) rmSync(file + ext, { force: true })
  precompress(scripts)
  sentryCli(['sourcemaps', 'upload', '--release', release, assets])
  console.log(`${dist}: uploaded ${mapsIn(dist).length} maps as ${release}`)
}

function main(argv: string[]): number {
  const deleteOnly = argv.includes('--delete-only')
  const wanted = argv.filter((a) => !a.startsWith('--'))
  const editions = findEditions(ROOT).filter((e) => !wanted.length || wanted.includes(e.id))
  const dists = editions.map((e) => join(ROOT, e.dir, 'dist')).filter((d) => existsSync(d))
  if (!dists.length) {
    console.error(
      'No built edition found. Build first: NASIKO_SOURCEMAPS=hidden node scripts/app.ts build',
    )
    return 1
  }
  const missing = ['SENTRY_AUTH_TOKEN', 'SENTRY_ORG', 'SENTRY_PROJECT'].filter(
    (k) => !process.env[k],
  )
  if (!deleteOnly && missing.length) {
    console.error(`Set ${missing.join(', ')} to upload, or pass --delete-only.`)
    return 1
  }
  let failed = false
  for (const dist of dists) {
    try {
      if (!deleteOnly) {
        if (!mapsIn(dist).length)
          throw new Error('no source maps: build with NASIKO_SOURCEMAPS=hidden')
        upload(dist)
      }
    } catch (err) {
      console.error(`${dist}: ${err instanceof Error ? err.message : String(err)}`)
      failed = true
    } finally {
      for (const map of mapsIn(dist)) rmSync(map)
    }
    if (mapsIn(dist).length) {
      console.error(`${dist}: source maps are still there; do not embed this build`)
      failed = true
    }
  }
  return failed ? 1 : 0
}

if (import.meta.url === `file://${process.argv[1]}`) process.exit(main(process.argv.slice(2)))
