// @vitest-environment node
import { strToU8, zipSync, type Zippable } from 'fflate'
import { describe, expect, it } from 'vitest'
import { nameFromFile, nameProblem } from './name'
import { checkZip, ENTRY_CAP, tomlVersion } from './zipcheck'

const zip = (files: Record<string, string | Uint8Array>, level: 0 | 6 = 6) =>
  new Blob([
    zipSync(
      Object.fromEntries(
        Object.entries(files).map(([k, v]) => [
          k,
          [typeof v === 'string' ? strToU8(v) : v, { level }],
        ]),
      ) as Zippable,
    ),
  ])

const GOOD = {
  Dockerfile: 'FROM python:3.12-slim\nCOPY . /app\n',
  'main.py': 'print("hi")\n',
  'AgentCard.json': JSON.stringify({ name: 'support-bot', version: 'v1.2.0' }),
}

describe('checkZip (eng review R5: tail read, capped entries)', () => {
  it('passes a well-formed agent and reads name and version from AgentCard.json (v stripped)', async () => {
    for (const level of [0, 6] as const) {
      const r = await checkZip(zip(GOOD, level))
      expect(r).toMatchObject({
        readable: true,
        version: '1.2.0',
        cardName: 'support-bot',
        topFolder: null,
      })
      expect(Object.values(r.items).map((i) => i.state)).toEqual(['pass', 'pass', 'pass', 'pass'])
    }
  })

  it('falls back to pyproject.toml ([project] then [tool.poetry]) and Cargo.toml like the server', async () => {
    const { 'AgentCard.json': _card, ...rest } = GOOD
    expect(
      (
        await checkZip(
          zip({ ...rest, 'pyproject.toml': '[tool.poetry]\nname = "x"\nversion = "0.4.1"\n' }),
        )
      ).version,
    ).toBe('0.4.1')
    expect(
      (await checkZip(zip({ ...rest, 'Cargo.toml': '[package]\nversion = "3.0.0"\n' }))).version,
    ).toBe('3.0.0')
    const none = await checkZip(zip(rest))
    expect(none.version).toBeNull()
    expect(none.items.version.state).toBe('unknown')
  })

  // Corrected in the /ship review: the server lifts a single top folder (flatten_single_top_level_dir), so a zipped
  // folder (Finder's Compress, GitHub's Download ZIP) passes; the earlier rule failed it.
  it('accepts a single top folder, __MACOSX, .DS_Store and ./ entries like the server', async () => {
    const nested = await checkZip(
      zip({
        ...Object.fromEntries(Object.entries(GOOD).map(([k, v]) => [`my-agent/${k}`, v])),
        '__MACOSX/my-agent/._main.py': 'x',
        '.DS_Store': 'x',
      }),
    )
    expect(nested.topFolder).toBe('my-agent/')
    expect(Object.values(nested.items).map((i) => i.state)).toEqual([
      'pass',
      'pass',
      'pass',
      'pass',
    ])
    expect(nested.version).toBe('1.2.0')
    const dotted = await checkZip(
      zip(Object.fromEntries(Object.entries(GOOD).map(([k, v]) => [`./${k}`, v]))),
    )
    expect(dotted.items.dockerfile.state).toBe('pass')
    // A second top-level entry, even an empty folder, stops the server lifting the wrapper.
    const twoTops = await checkZip(
      zip({
        ...Object.fromEntries(Object.entries(GOOD).map(([k, v]) => [`my-agent/${k}`, v])),
        'logs/': new Uint8Array(),
      }),
    )
    expect(twoTops.topFolder).toBeNull()
    expect(twoTops.items.dockerfile.state).toBe('fail')
    const missing = await checkZip(zip({ 'my-agent/main.py': 'x' }))
    expect(missing.items.dockerfile).toMatchObject({ state: 'fail' })
  })

  it('fails a Dockerfile without a FROM line (FROM must be followed by a space, as the server checks)', async () => {
    expect(
      (await checkZip(zip({ ...GOOD, Dockerfile: '# no base\nRUN echo\n' }))).items.dockerfile
        .state,
    ).toBe('fail')
    expect(
      (await checkZip(zip({ ...GOOD, Dockerfile: '  FROM python\n' }))).items.dockerfile.state,
    ).toBe('pass')
    expect(
      (await checkZip(zip({ ...GOOD, Dockerfile: 'FROMpython\n' }))).items.dockerfile.state,
    ).toBe('fail')
  })

  it('accepts any server entrypoint and rejects a non-x.y.z version', async () => {
    const { 'main.py': _m, ...rest } = GOOD
    expect((await checkZip(zip({ ...rest, 'src/__main__.py': '' }))).items.entrypoint.state).toBe(
      'pass',
    )
    const bad = await checkZip(
      zip({ ...GOOD, 'AgentCard.json': JSON.stringify({ version: 'latest' }) }),
    )
    expect(bad.items.version.state).toBe('fail')
    expect(bad.version).toBeNull()
  })

  it('never inflates an entry past the 64 KB cap (a crafted zip cannot blow up memory)', async () => {
    const big = 'FROM python\n' + 'x'.repeat(ENTRY_CAP + 10)
    const r = await checkZip(zip({ ...GOOD, Dockerfile: big }))
    expect(r.items.dockerfile.state).toBe('unknown')
  })

  it('makes the checklist advisory for anything that is not a readable zip', async () => {
    for (const blob of [
      new Blob(['not a zip at all']),
      new Blob([]),
      new Blob([new Uint8Array(100)]),
    ]) {
      const r = await checkZip(blob)
      expect(r.readable).toBe(false)
      expect(r.items.dockerfile.state).toBe('unknown')
    }
    // A truncated zip (the central directory points past the end).
    const whole = zipSync({ a: strToU8('x') })
    expect((await checkZip(new Blob([whole.slice(10)]))).readable).toBe(false)
  })

  it('fails the size item past 100 MB without reading the file', async () => {
    const huge = { size: 100 * 1024 * 1024 + 1, slice: () => new Blob([]) } as unknown as Blob
    const r = await checkZip(huge)
    expect(r.items.size.state).toBe('fail')
  })
})

describe('tomlVersion', () => {
  it('reads only the named tables', () => {
    expect(
      tomlVersion('[tool.black]\nversion = "9"\n[project]\nversion = "1.0.0"', ['project']),
    ).toBe('1.0.0')
    expect(tomlVersion('[project]\nname = "x"', ['project'])).toBeNull()
  })
})

describe('names (validate_version_tag rules)', () => {
  it('explains each rule', () => {
    expect(nameProblem('support-bot')).toBeNull()
    expect(nameProblem('')).toMatch(/1–128/)
    expect(nameProblem('x'.repeat(129))).toMatch(/1–128/)
    expect(nameProblem('-bot')).toMatch(/Start with/)
    expect(nameProblem('my bot')).toMatch(/only letters/)
  })

  it('derives a valid name from a file name', () => {
    expect(nameFromFile('My Agent (v2).zip')).toBe('My-Agent-v2')
    expect(nameFromFile('--x.zip')).toBe('x')
    expect(nameProblem(nameFromFile('support-bot.zip'))).toBeNull()
    expect(nameFromFile('.zip')).toBe('')
  })

  it('treats a zip64 archive as unreadable (advisory), never as real offsets (R5)', async () => {
    const z = zipSync({ Dockerfile: strToU8('FROM x\n') })
    // The EOCD's total-entries field set to the zip64 sentinel.
    new DataView(z.buffer, z.byteOffset).setUint16(z.length - 22 + 10, 0xffff, true)
    const r = await checkZip(new Blob([z]))
    expect(r.readable).toBe(false)
    expect(r.items.dockerfile.state).toBe('unknown')
  })
})
