import { MutationCache, QueryClient } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/lib/api/client'
import {
  cancelUpload,
  clearUploads,
  dismissUpload,
  setXhrFactory,
  startUpload,
  subscribeUploads,
  uploadFor,
  UPLOAD_PATH,
  type XhrFactory,
} from './uploads'

/** A stand-in XMLHttpRequest the test drives (eng review R7). */
class FakeXhr {
  static last: FakeXhr | null = null
  method = ''
  url = ''
  status = 0
  responseText = ''
  withCredentials = false
  headers: Record<string, string> = {}
  body: FormData | null = null
  aborted = false
  upload: {
    onprogress: ((e: { loaded: number; total: number; lengthComputable: boolean }) => void) | null
  } = { onprogress: null }
  onload: (() => void) | null = null
  onerror: (() => void) | null = null
  onabort: (() => void) | null = null
  constructor() {
    FakeXhr.last = this
  }
  open(method: string, url: string) {
    this.method = method
    this.url = url
  }
  setRequestHeader(k: string, v: string) {
    this.headers[k] = v
  }
  send(body: FormData) {
    this.body = body
  }
  abort() {
    this.aborted = true
    this.onabort?.()
  }
  progress(loaded: number, total: number) {
    this.upload.onprogress?.({ loaded, total, lengthComputable: true })
  }
  respond(status: number, body: string) {
    this.status = status
    this.responseText = body
    this.onload?.()
  }
}

/** The mutation starts its XHR a microtask later. */
async function xhr(): Promise<FakeXhr> {
  await vi.waitFor(() => expect(FakeXhr.last).not.toBeNull())
  return FakeXhr.last!
}

let prev: XhrFactory | null = null
const USER = 'u1'
const file = new Blob(['zip bytes'])

function setup() {
  prev = setXhrFactory(() => new FakeXhr() as unknown as XMLHttpRequest)
  const onError = vi.fn()
  const qc = new QueryClient({ mutationCache: new MutationCache({ onError }) })
  return { qc, onError }
}

afterEach(() => {
  clearUploads()
  if (prev) setXhrFactory(prev)
  FakeXhr.last = null
})

describe('upload registry (eng review R1, R2, R3, R7)', () => {
  it('sends the multipart fields the server reads, only the touched ones', async () => {
    const { qc } = setup()
    void startUpload(qc, USER, file, 'bot.zip', {
      name: 'bot',
      version: '1.0.0',
      ports: '8000,9000',
      env: { A: '1' },
      inboundFormat: 'openai',
    })
    const x = await xhr()
    expect([x.method, x.url, x.withCredentials]).toEqual(['POST', UPLOAD_PATH, true])
    expect(
      Object.fromEntries(
        [...x.body!.entries()].map(([k, v]) => [k, typeof v === 'string' ? v : 'FILE']),
      ),
    ).toEqual({
      name: 'bot',
      version_tag: '1.0.0',
      ports: '8000,9000',
      env: '{"A":"1"}',
      inbound_format: 'openai',
      file: 'FILE',
    })
    // Storage is tri-state on the server: sent only when touched.
    expect(x.body!.has('writable')).toBe(false)
  })

  it('reports progress, then the build id from the 202', async () => {
    const { qc } = setup()
    const seen: number[] = []
    const off = subscribeUploads(() => {
      const s = uploadFor(USER)
      if (s?.phase === 'uploading') seen.push(s.loaded)
    })
    const done = startUpload(qc, USER, file, 'bot.zip', { name: 'bot' })
    ;(await xhr()).progress(10, 100)
    ;(await xhr()).progress(100, 100)
    ;(await xhr()).respond(
      202,
      JSON.stringify({
        data: { build_id: 'b1', agent_id: 'a1', status: 'queued' },
        status_code: 202,
        message: 'ok',
      }),
    )
    await expect(done).resolves.toMatchObject({ phase: 'done', buildId: 'b1', agentId: 'a1' })
    expect(seen).toEqual([0, 10, 100])
    off()
  })

  it('keeps going with no page mounted and survives a re-attach (R1)', async () => {
    const { qc } = setup()
    const done = startUpload(qc, USER, file, 'bot.zip', { name: 'bot' })
    // A page that mounts later reads the same state.
    expect(uploadFor(USER)).toMatchObject({ phase: 'uploading', fileName: 'bot.zip' })
    // A second start while uploading doesn't open another request.
    const first = await xhr()
    void startUpload(qc, USER, file, 'other.zip', { name: 'other' })
    expect(FakeXhr.last).toBe(first)
    first.respond(202, JSON.stringify({ data: { build_id: 'b2', agent_id: 'a2' } }))
    await done
    expect(uploadFor(USER)).toMatchObject({ phase: 'done', buildId: 'b2' })
    dismissUpload(USER)
    expect(uploadFor(USER)).toBeNull()
  })

  it.each([401, 409, 413, 500])(
    'turns a %i into an ApiError through the QueryClient mutation cache (R2)',
    async (status) => {
      const { qc, onError } = setup()
      const done = startUpload(qc, USER, file, 'bot.zip', { name: 'bot' })
      ;(await xhr()).respond(
        status,
        status === 409
          ? "version 1.0.0 already exists in this agent's history — choose a new version"
          : 'nope',
      )
      const s = await done
      expect(s.phase).toBe('failed')
      const err = (s as { error: ApiError }).error
      expect(err).toBeInstanceOf(ApiError)
      expect(err.status).toBe(status)
      // The app's MutationCache onError sees it: that's where a 401 becomes /login?expired=true.
      expect(onError).toHaveBeenCalledOnce()
      expect(onError.mock.calls[0]![0]).toBe(err)
    },
  )

  it('maps a network failure and a 202 without a build id to errors', async () => {
    const { qc } = setup()
    const a = startUpload(qc, USER, file, 'bot.zip', { name: 'bot' })
    ;(await xhr()).onerror!()
    expect(await a).toMatchObject({ phase: 'failed', error: { status: 502 } })
    FakeXhr.last = null
    const b = startUpload(qc, USER, file, 'bot.zip', { name: 'bot' })
    ;(await xhr()).respond(202, '{"data":{}}')
    expect(await b).toMatchObject({ phase: 'failed' })
  })

  it('cancels by aborting the XHR', async () => {
    const { qc, onError } = setup()
    const done = startUpload(qc, USER, file, 'bot.zip', { name: 'bot' })
    await xhr()
    cancelUpload(USER)
    expect(FakeXhr.last!.aborted).toBe(true)
    expect(await done).toMatchObject({ phase: 'cancelled' })
    // An abort reaches the shared handler as an AbortError, never as an ApiError (so no 401 path can fire).
    expect(onError).toHaveBeenCalledOnce()
    expect(onError.mock.calls[0]![0]).toMatchObject({ name: 'AbortError' })
    expect(onError.mock.calls[0]![0]).not.toBeInstanceOf(ApiError)
  })

  it('asks before closing the tab only while uploading', async () => {
    const { qc } = setup()
    const add = vi.spyOn(window, 'addEventListener')
    const remove = vi.spyOn(window, 'removeEventListener')
    const done = startUpload(qc, USER, file, 'bot.zip', { name: 'bot' })
    expect(add.mock.calls.filter(([t]) => t === 'beforeunload')).toHaveLength(1)
    ;(await xhr()).respond(202, JSON.stringify({ data: { build_id: 'b3', agent_id: 'a3' } }))
    await done
    expect(remove.mock.calls.filter(([t]) => t === 'beforeunload')).toHaveLength(1)
  })

  it('forgets everything on sign-out, and a late answer is not written back (R3)', async () => {
    const { qc } = setup()
    const done = startUpload(qc, USER, file, 'bot.zip', { name: 'bot' })
    const x = await xhr()
    clearUploads()
    expect(x.aborted).toBe(true)
    expect(uploadFor(USER)).toBeNull()
    await done
    expect(uploadFor(USER)).toBeNull()
  })
})

describe('registry imports in the registry (eng review R4)', () => {
  it('runs as a QueryClient mutation (a 401 reaches the shared handler) and records the result', async () => {
    const { qc, onError } = setup()
    const { importFor, startImport } = await import('./uploads')
    const ok = await startImport(qc, USER, 'registry.nasiko.dev/a/b:1.0.0', async () => ({
      agent_id: 'a1',
      build_id: 'b1',
      container_name: 'c',
      status: 'success',
    }))
    expect(ok).toMatchObject({ phase: 'done', buildId: 'b1', containerName: 'c' })
    const failed = await startImport(qc, USER, 'x', async () => {
      throw new ApiError(401, null, '/api/import/registry', 'nope')
    })
    expect(failed).toMatchObject({ phase: 'failed', error: { status: 401 } })
    expect(onError).toHaveBeenCalledOnce()
    expect(importFor(USER)?.phase).toBe('failed')
  })

  it('is forgotten on sign-out even if the server answers later (R3)', async () => {
    const { qc } = setup()
    const { importFor, startImport } = await import('./uploads')
    let answer!: (v: { agent_id: string; status: string }) => void
    const p = startImport(
      qc,
      USER,
      'r',
      () =>
        new Promise((r) => {
          answer = r
        }),
    )
    expect(importFor(USER)?.phase).toBe('importing')
    clearUploads()
    await vi.waitFor(() => expect(answer).toBeTypeOf('function'))
    answer({ agent_id: 'late', status: 'success' })
    await p
    expect(importFor(USER)).toBeNull()
  })
})
