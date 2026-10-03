/**
 * The in-browser zip check (plans/feat-deploy.md §4.1; design review 5, eng review R5 / D6, recorded explicitly).
 *
 * Only the tail of the file is read (`Blob.slice`: the end-of-central-directory record, then the central directory),
 * then only the entries the checklist needs (AgentCard.json, pyproject.toml, Cargo.toml, Dockerfile), each capped at
 * 64 KB and inflated with fflate. The whole zip is never loaded into memory. A zip64 or unreadable archive makes the
 * checklist advisory; the server stays the judge (`validate_agent_zip`, nasiko-cloud-rs `2d6178e4`).
 */
import { inflateSync } from 'fflate'
import { checkCopy } from './checkCopy'
import type { ChecklistItem } from './errors'
import { parseVersion } from './version'

export const MAX_ZIP_BYTES = 100 * 1024 * 1024
export const MAX_FILES = 1_000
export const MAX_UNZIPPED_BYTES = 200 * 1024 * 1024
export const ENTRY_CAP = 64 * 1024
const ENTRYPOINTS = ['main.py', 'src/main.py', '__main__.py', 'src/__main__.py'] as const

export type ItemState = 'pass' | 'fail' | 'unknown'
/** The checklist's order (design review 4). */
export const CHECKLIST_ITEMS: readonly ChecklistItem[] = [
  'dockerfile',
  'entrypoint',
  'version',
  'size',
]

export interface ItemResult {
  state: ItemState
  /** Why it failed, in plain words. */
  detail?: string
}

export type ItemResults = Record<ChecklistItem, ItemResult>

export interface ZipCheck {
  /** False: the archive couldn't be read here (zip64, corrupt, not a zip); the checklist is advisory. */
  readable: boolean
  items: Record<ChecklistItem, ItemResult>
  /** The version the server would detect (AgentCard.json → pyproject.toml → Cargo.toml), `v` stripped. */
  version: string | null
  /** AgentCard.json's `name`, when set. */
  cardName: string | null
  /** A single top-level folder holding everything (the server needs the files at the root). */
  topFolder: string | null
}

interface Entry {
  name: string
  method: number
  compressedSize: number
  size: number
  localOffset: number
}

const EOCD_SIG = 0x06054b50
const CD_SIG = 0x02014b50
const LOCAL_SIG = 0x04034b50

async function bytes(blob: Blob, start: number, end: number): Promise<Uint8Array> {
  return new Uint8Array(await blob.slice(start, end).arrayBuffer())
}

/** The central directory's entries, or null when the tail isn't a plain (non-zip64) zip. */
async function readDirectory(file: Blob): Promise<Entry[] | null> {
  const tailLen = Math.min(file.size, 22 + 0xffff)
  const tail = await bytes(file, file.size - tailLen, file.size)
  const view = new DataView(tail.buffer, tail.byteOffset, tail.byteLength)
  let eocd = -1
  for (let i = tail.length - 22; i >= 0; i--) {
    if (view.getUint32(i, true) === EOCD_SIG) {
      eocd = i
      break
    }
  }
  if (eocd < 0) return null
  const count = view.getUint16(eocd + 10, true)
  const cdSize = view.getUint32(eocd + 12, true)
  const cdOffset = view.getUint32(eocd + 16, true)
  // zip64 markers: the real values live in a zip64 record this check doesn't read.
  if (count === 0xffff || cdSize === 0xffffffff || cdOffset === 0xffffffff) return null
  if (cdOffset + cdSize > file.size) return null
  const cd = await bytes(file, cdOffset, cdOffset + cdSize)
  const dv = new DataView(cd.buffer, cd.byteOffset, cd.byteLength)
  const dec = new TextDecoder()
  const entries: Entry[] = []
  let p = 0
  for (let n = 0; n < count; n++) {
    if (p + 46 > cd.length || dv.getUint32(p, true) !== CD_SIG) return null
    const nameLen = dv.getUint16(p + 28, true)
    const extraLen = dv.getUint16(p + 30, true)
    const commentLen = dv.getUint16(p + 32, true)
    entries.push({
      method: dv.getUint16(p + 10, true),
      compressedSize: dv.getUint32(p + 20, true),
      size: dv.getUint32(p + 24, true),
      localOffset: dv.getUint32(p + 42, true),
      name: dec.decode(cd.subarray(p + 46, p + 46 + nameLen)),
    })
    p += 46 + nameLen + extraLen + commentLen
  }
  return entries
}

/** One entry's text (≤ 64 KB unzipped), or null when it's bigger, compressed another way, or unreadable. */
async function readText(file: Blob, e: Entry): Promise<string | null> {
  if (e.size > ENTRY_CAP || e.compressedSize > ENTRY_CAP) return null
  const head = await bytes(file, e.localOffset, e.localOffset + 30)
  const hv = new DataView(head.buffer, head.byteOffset, head.byteLength)
  if (head.length < 30 || hv.getUint32(0, true) !== LOCAL_SIG) return null
  const start = e.localOffset + 30 + hv.getUint16(26, true) + hv.getUint16(28, true)
  const data = await bytes(file, start, start + e.compressedSize)
  try {
    if (e.method === 0) return new TextDecoder().decode(data)
    if (e.method === 8)
      return new TextDecoder().decode(inflateSync(data, { out: new Uint8Array(e.size) }))
  } catch {
    return null
  }
  return null
}

/** `parse_toml_version`: the first `version = "…"` inside one of the named tables. */
export function tomlVersion(text: string, tables: readonly string[]): string | null {
  let current = ''
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim()
    const header = /^\[([^\]]+)\]$/.exec(line)
    if (header) {
      current = (header[1] ?? '').trim()
      continue
    }
    if (!tables.includes(current)) continue
    const m = /^version\s*=\s*["']([^"']+)["']/.exec(line)
    if (m?.[1]) return m[1]
  }
  return null
}

const UNKNOWN: ItemResult = { state: 'unknown' }

/** Check a zip the way `validate_agent_zip` will, without uploading it. Never throws. */
export async function checkZip(file: Blob): Promise<ZipCheck> {
  const sizeOk = file.size <= MAX_ZIP_BYTES
  const advisory = (): ZipCheck => ({
    readable: false,
    items: {
      dockerfile: UNKNOWN,
      entrypoint: UNKNOWN,
      version: UNKNOWN,
      size: sizeOk
        ? { state: 'pass' }
        : { state: 'fail', detail: checkCopy.overSize(MAX_ZIP_BYTES / 2 ** 20) },
    },
    version: null,
    cardName: null,
    topFolder: null,
  })
  let entries: Entry[] | null
  try {
    entries = await readDirectory(file)
  } catch {
    entries = null
  }
  if (!entries) return advisory()

  // Like `extract_zip_reader` (oss/utils/src/zip.rs): `./` is ignored, `__MACOSX/` and a root `.DS_Store` are dropped, and
  // a single top-level folder is lifted to the root (`flatten_single_top_level_dir`), so Finder's Compress and GitHub's
  // Download ZIP work as they are.
  const clean = (n: string) => n.replace(/^(\.\/)+/, '')
  const cleaned = entries.map((e) => ({ ...e, name: clean(e.name) })).filter((e) => e.name)
  // The 200 MB cap counts every extracted file, __MACOSX and .DS_Store included.
  const unzipped = cleaned.filter((e) => !e.name.endsWith('/')).reduce((n, e) => n + e.size, 0)
  // What's left on disk after the server drops __MACOSX and the root .DS_Store: files and (empty) directories alike.
  const kept = cleaned.filter((e) => !e.name.startsWith('__MACOSX/') && e.name !== '.DS_Store')
  const files = kept.filter((e) => !e.name.endsWith('/'))
  const tops = new Set(kept.map((e) => e.name.split('/')[0]))
  // Lifted only when that single top-level entry is a folder.
  const topFolder =
    tops.size === 1 && kept.every((e) => e.name.includes('/')) ? `${[...tops][0]}/` : null
  const byName = new Map(files.map((e) => [topFolder ? e.name.slice(topFolder.length) : e.name, e]))

  let size: ItemResult = { state: 'pass' }
  if (!sizeOk) size = { state: 'fail', detail: checkCopy.overSize(MAX_ZIP_BYTES / 2 ** 20) }
  else if (entries.length > MAX_FILES)
    size = { state: 'fail', detail: checkCopy.overFiles(MAX_FILES) }
  else if (unzipped > MAX_UNZIPPED_BYTES)
    size = { state: 'fail', detail: checkCopy.overUnzipped(MAX_UNZIPPED_BYTES / 2 ** 20) }

  let dockerfile: ItemResult
  const df = byName.get('Dockerfile')
  if (!df) dockerfile = { state: 'fail', detail: checkCopy.noDockerfile }
  else {
    const text = await readText(file, df)
    if (text === null) dockerfile = { state: 'unknown' }
    else
      dockerfile = text.split(/\r?\n/).some((l) => l.trimStart().startsWith('FROM '))
        ? { state: 'pass' }
        : { state: 'fail', detail: checkCopy.noFrom }
  }

  const entrypoint: ItemResult = ENTRYPOINTS.some((p) => byName.has(p))
    ? { state: 'pass' }
    : { state: 'fail', detail: checkCopy.noEntrypoint }

  let version: string | null = null
  let cardName: string | null = null
  const card = byName.get('AgentCard.json')
  if (card) {
    const text = await readText(file, card)
    try {
      const json = text ? (JSON.parse(text) as { version?: unknown; name?: unknown }) : null
      if (json && typeof json.version === 'string') version = json.version.replace(/^v/, '')
      if (json && typeof json.name === 'string') cardName = json.name
    } catch {
      // A broken AgentCard.json: the server skips it the same way and tries the next file.
    }
  }
  for (const [path, tables] of [
    ['pyproject.toml', ['project', 'tool.poetry']],
    ['Cargo.toml', ['package']],
  ] as const) {
    if (version) break
    const e = byName.get(path)
    const text = e ? await readText(file, e) : null
    if (text) version = tomlVersion(text, tables)
  }
  const versionItem: ItemResult =
    version === null
      ? { state: 'unknown', detail: checkCopy.noVersion }
      : parseVersion(version)
        ? { state: 'pass' }
        : { state: 'fail', detail: checkCopy.badVersion(version) }

  return {
    readable: true,
    items: { dockerfile, entrypoint, version: versionItem, size },
    version: version && parseVersion(version) ? version : null,
    cardName,
    topFolder,
  }
}
