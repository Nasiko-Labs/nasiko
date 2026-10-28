// @vitest-environment node
import { randomBytes } from 'node:crypto'
import { existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { brotliDecompressSync, gunzipSync } from 'node:zlib'
import { describe, expect, it } from 'vitest'
import { precompress } from '../../../scripts/lib/precompress.ts'

describe('precompress (the build writes the variants the Rust servers serve)', () => {
  it('writes .br and .gz beside a text asset that compresses, and skips small, incompressible or binary files', () => {
    const dir = mkdtempSync(join(tmpdir(), 'precompress-'))
    const js = join(dir, 'app.js')
    const text = 'export const greeting = "hello";\n'.repeat(200)
    writeFileSync(js, text)
    const small = join(dir, 'tiny.css')
    writeFileSync(small, 'a{}')
    const noise = join(dir, 'noise.json')
    writeFileSync(noise, randomBytes(4096))
    const font = join(dir, 'font.woff2')
    writeFileSync(font, Buffer.alloc(4096, 7))

    const written = precompress([js, small, noise, font])

    expect(brotliDecompressSync(readFileSync(`${js}.br`)).toString()).toBe(text)
    expect(gunzipSync(readFileSync(`${js}.gz`)).toString()).toBe(text)
    expect(existsSync(`${small}.br`)).toBe(false)
    expect(existsSync(`${noise}.br`) || existsSync(`${noise}.gz`)).toBe(false)
    expect(existsSync(`${font}.br`)).toBe(false)
    expect(written).toEqual({ br: 1, gz: 1, skipped: 1 })
  })
})
