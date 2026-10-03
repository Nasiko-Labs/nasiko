/** Ship-audit gap tests for deploy's pure logic and module state (branches the feature tests don't reach). */
import { QueryClient } from '@tanstack/react-query'
import { strToU8, zipSync, type Zippable } from 'fflate'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { readEnv } from '@/lib/env'
import { buildsSearchSchema, deploySearchSchema } from './search'
import {
  clearUploads,
  dismissUpload,
  setXhrFactory,
  startUpload,
  uploadFor,
  type XhrFactory,
} from './uploads'
import { checkZip, MAX_FILES } from './zipcheck'

const zip = (files: Record<string, string>) =>
  new Blob([
    zipSync(Object.fromEntries(Object.entries(files).map(([k, v]) => [k, strToU8(v)])) as Zippable),
  ])

describe('VITE_NASIKO_MOCK=deploy (plans/feat-deploy.md §9)', () => {
  it('needs agents: alone it is dropped with a warning, with agents it stays', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const live = (mock: string) =>
      readEnv({ VITE_NASIKO_API_MODE: 'live', VITE_NASIKO_MOCK: mock, DEV: true }).partialMocks
    expect(live('deploy')).toEqual([])
    expect(warn).toHaveBeenCalledWith(expect.stringContaining('"deploy" needs "agents"'))
    expect(live('deploy,agents').sort()).toEqual(['agents', 'deploy'])
    warn.mockRestore()
  })
})

describe('checkZip edges', () => {
  it('fails the size item past the file-count limit', async () => {
    const many: Record<string, string> = { Dockerfile: 'FROM python:3.12\n', 'main.py': '' }
    for (let i = 0; i < MAX_FILES; i++) many[`f/${i}.txt`] = ''
    const r = await checkZip(zip(many))
    expect(r.readable).toBe(true)
    expect(r.items.size.state).toBe('fail')
    expect(r.items.dockerfile.state).toBe('pass')
  })

  it('skips a broken AgentCard.json and reads the version from pyproject.toml, like the server', async () => {
    const r = await checkZip(
      zip({
        Dockerfile: 'FROM x\n',
        'main.py': '',
        'AgentCard.json': '{not json',
        'pyproject.toml': '[project]\nversion = "2.1.0"\n',
      }),
    )
    expect(r.version).toBe('2.1.0')
    expect(r.cardName).toBeNull()
    expect(r.items.version.state).toBe('pass')
  })
})

describe('URL state falls back instead of throwing', () => {
  it('drops junk Builds and Deploy params', () => {
    expect(buildsSearchSchema.parse({ status: 'nope', page: -3, q: 'x'.repeat(500) })).toEqual({
      status: undefined,
      page: undefined,
      q: undefined,
    })
    expect(
      deploySearchSchema.parse({
        method: 'ftp',
        repo: 'no-slash',
        name: 'a b; rm -rf',
        version: 'v'.repeat(41),
      }),
    ).toEqual({ method: undefined, repo: undefined, name: undefined, version: undefined })
    expect(
      deploySearchSchema.parse({
        method: 'github',
        repo: 'acme/bot',
        name: 'bot',
        version: '1.0.0',
      }),
    ).toEqual({ method: 'github', repo: 'acme/bot', name: 'bot', version: '1.0.0' })
  })
})

describe('upload registry: double submit', () => {
  let prev: XhrFactory | null = null
  afterEach(() => {
    clearUploads()
    if (prev) setXhrFactory(prev)
  })

  it('a second start while uploading returns the running upload and sends nothing; dismiss waits for it to finish', async () => {
    const made: { abort: () => void }[] = []
    prev = setXhrFactory(() => {
      const x = {
        open() {},
        setRequestHeader() {},
        send() {},
        abort() {
          this.onabort?.()
        },
        upload: {},
        withCredentials: false,
        onabort: null as null | (() => void),
      }
      made.push(x)
      return x as unknown as XMLHttpRequest
    })
    const qc = new QueryClient()
    const first = startUpload(qc, 'u1', new Blob(['a']), 'a.zip', { name: 'a' })
    await vi.waitFor(() => expect(made).toHaveLength(1))
    const second = await startUpload(qc, 'u1', new Blob(['b']), 'b.zip', { name: 'b' })
    expect(second).toMatchObject({ phase: 'uploading', fileName: 'a.zip' })
    expect(made).toHaveLength(1)
    dismissUpload('u1')
    expect(uploadFor('u1')).toMatchObject({ phase: 'uploading' })
    made[0]!.abort()
    expect(await first).toMatchObject({ phase: 'cancelled' })
    dismissUpload('u1')
    expect(uploadFor('u1')).toBeNull()
  })
})
