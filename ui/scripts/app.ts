// One runner for every edition (docs/lab-vs-react-migration-review.md §10.6):
//   node scripts/app.ts dev [edition] [-- vite args]   (default: oss)
//   node scripts/app.ts build [edition]                (default: every edition found)
//   node scripts/app.ts preview [edition]
//   node scripts/app.ts typecheck                      (one tsc program per edition, plus tooling and e2e)
// Editions are found by folder (scripts/editions.ts), so this file names none but the core's own.
import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { findEditions } from './editions.ts'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const editions = findEditions(ROOT)
const [cmd, name, ...rest] = process.argv.slice(2)

function run(bin: string, args: string[]) {
  const r = spawnSync(bin, args, { cwd: ROOT, stdio: 'inherit', shell: true })
  if (r.error) console.error(r.error)
  if (r.status !== 0) process.exit(r.status ?? 1)
}
function pick(fallback: 'oss' | 'all') {
  if (name && !name.startsWith('-')) {
    const e = editions.find((x) => x.id === name)
    if (!e) {
      console.error(
        `No edition "${name}" in this checkout (found: ${editions.map((x) => x.id).join(', ')})`,
      )
      process.exit(2)
    }
    return [e]
  }
  return fallback === 'all' ? editions : editions.filter((e) => e.id === 'oss')
}
const extra = [...(name?.startsWith('-') ? [name] : []), ...rest].filter((a) => a !== '--')

switch (cmd) {
  case 'dev':
  case 'preview': {
    const [e] = pick('oss')
    run('npx', [
      'vite',
      ...(cmd === 'preview' ? ['preview'] : []),
      '--config',
      `${e!.dir}/vite.config.ts`,
      ...extra,
    ])
    break
  }
  case 'build':
    for (const e of pick('all'))
      run('npx', ['vite', 'build', '--config', `${e.dir}/vite.config.ts`, ...extra])
    break
  case 'typecheck':
    run('npx', [
      'tsc',
      '-b',
      ...editions.map((e) => `${e.dir}/tsconfig.json`),
      // An edition's own browser specs (`<app>/e2e`), when it has them.
      ...editions.map((e) => `${e.dir}/e2e/tsconfig.json`).filter((p) => existsSync(join(ROOT, p))),
      'tsconfig.node.json',
      'e2e/tsconfig.json',
    ])
    break
  default:
    console.error('usage: node scripts/app.ts dev|build|preview|typecheck [edition]')
    process.exit(2)
}
