// Upload a release build's source maps to Sentry, then delete them, so stack traces are readable in Sentry while no
// map is ever embedded in a binary or served from /assets (a map would publish private source).
//
//   NASIKO_SOURCEMAPS=hidden node scripts/app.ts build [edition]
//   SENTRY_AUTH_TOKEN=… SENTRY_ORG=nasiko-labs SENTRY_PROJECT=nasiko-ui node scripts/sentry-sourcemaps.ts [edition…]
//   node scripts/sentry-sourcemaps.ts --delete-only [edition…]      (no upload: just make the dist safe to embed)
//
// A build whose upload must wait for a later, approved step (the EE release candidate: its build job holds no
// secret, its publish job does) splits the work in two:
//
//   node scripts/sentry-sourcemaps.ts --keep-maps=<dir> [edition…]  (inject, copy scripts + maps to <dir>; no token)
//   SENTRY_AUTH_TOKEN=… … node scripts/sentry-sourcemaps.ts --upload-kept=<dir>
//
// Per edition: sentry-cli injects a debug ID into each script and its map (Sentry pairs them by that ID, so no file
// naming or release matching can go wrong), the changed scripts are compressed again (the servers serve the .br/.gz
// variants, which must carry the same ID), the maps are uploaded under the build's release name
// (dist/.vite/sentry-release.txt) or moved out, and every map is deleted. The delete runs even when an earlier step
// fails. The auth token is read from the environment only (CI's secret store): never pass it as an argument or write
// it to a file in the repository (no .sentryclirc, no .env.sentry-build-plugin).
import { execFileSync } from 'node:child_process'
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'
import { findEditions } from './editions.ts'
import { precompress } from './lib/precompress.ts'

/** Pinned: this runs with CI's credentials. */
const SENTRY_CLI = '@sentry/cli@3.8.0'
const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const UPLOAD_ENV = ['SENTRY_AUTH_TOKEN', 'SENTRY_ORG', 'SENTRY_PROJECT']
const RELEASE_FILE = 'sentry-release.txt'

function walk(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) =>
    e.isDirectory() ? walk(join(dir, e.name)) : [join(dir, e.name)],
  )
}

const mapsIn = (dist: string) => walk(dist).filter((f) => /\.map(\.br|\.gz)?$/.test(f))
const releaseOf = (dir: string) => readFileSync(join(dir, RELEASE_FILE), 'utf8').trim()

function sentryCli(args: string[]) {
  execFileSync('npx', ['--yes', SENTRY_CLI, ...args], { stdio: 'inherit', env: process.env })
}

/** Debug IDs into every script and its map, then the scripts' .br/.gz again so every variant carries the same ID. */
function inject(dist: string) {
  const assets = join(dist, 'assets')
  sentryCli(['sourcemaps', 'inject', assets])
  const scripts = walk(assets).filter((f) => /\.m?js$/.test(f))
  for (const file of scripts) for (const ext of ['.br', '.gz']) rmSync(file + ext, { force: true })
  precompress(scripts)
}

function upload(assets: string, release: string) {
  sentryCli(['sourcemaps', 'upload', '--release', release, assets])
  console.log(`${assets}: uploaded ${mapsIn(assets).length} maps as ${release}`)
}

/**
 * The scripts and their maps (plain files only: the .br/.gz copies are never uploaded) and the release name, under
 * `<kept>/<id>`, so a later step can upload them exactly as `upload` would have.
 */
function keep(dist: string, kept: string) {
  const assets = join(dist, 'assets')
  for (const file of walk(assets).filter((f) => /\.(m?js|map)$/.test(f))) {
    const dest = join(kept, relative(assets, file))
    mkdirSync(dirname(dest), { recursive: true })
    cpSync(file, dest)
  }
  cpSync(join(dist, '.vite', RELEASE_FILE), join(kept, RELEASE_FILE))
  console.log(`${dist}: kept ${mapsIn(kept).length} maps in ${kept}`)
}

function missingUploadEnv(): string[] {
  return UPLOAD_ENV.filter((k) => !process.env[k])
}

function uploadKept(dir: string): number {
  const missing = missingUploadEnv()
  if (missing.length) {
    console.error(`Set ${missing.join(', ')} to upload.`)
    return 1
  }
  const builds = existsSync(dir)
    ? readdirSync(dir).filter((id) => existsSync(join(dir, id, RELEASE_FILE)))
    : []
  if (!builds.length) {
    console.error(`${dir}: no kept source maps (run --keep-maps first)`)
    return 1
  }
  for (const id of builds) upload(join(dir, id), releaseOf(join(dir, id)))
  return 0
}

function main(argv: string[]): number {
  const option = (name: string) =>
    argv.find((a) => a.startsWith(`--${name}=`))?.slice(name.length + 3) || undefined
  const uploadDir = option('upload-kept')
  if (uploadDir) return uploadKept(uploadDir)
  const keepDir = option('keep-maps')
  const deleteOnly = argv.includes('--delete-only')
  const wanted = argv.filter((a) => !a.startsWith('--'))
  const editions = findEditions(ROOT).filter((e) => !wanted.length || wanted.includes(e.id))
  const builds = editions
    .map((e) => ({ id: e.id, dist: join(ROOT, e.dir, 'dist') }))
    .filter((b) => existsSync(b.dist))
  if (!builds.length) {
    console.error(
      'No built edition found. Build first: NASIKO_SOURCEMAPS=hidden node scripts/app.ts build',
    )
    return 1
  }
  const missing = missingUploadEnv()
  if (!deleteOnly && !keepDir && missing.length) {
    console.error(`Set ${missing.join(', ')} to upload, or pass --delete-only.`)
    return 1
  }
  let failed = false
  for (const { id, dist } of builds) {
    try {
      if (!deleteOnly) {
        if (!mapsIn(dist).length)
          throw new Error('no source maps: build with NASIKO_SOURCEMAPS=hidden')
        inject(dist)
        if (keepDir) keep(dist, join(keepDir, id))
        else upload(join(dist, 'assets'), releaseOf(join(dist, '.vite')))
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
