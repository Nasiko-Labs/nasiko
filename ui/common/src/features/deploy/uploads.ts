/**
 * The upload registry (plans/feat-deploy.md §4.1; eng review R1, R2, R3, R7).
 *
 * - R1: an upload lives here, outside React, keyed by user, so moving around the app keeps it going and /deploy
 *   re-attaches to it. Only closing or reloading the tab asks first (`beforeunload` while uploading); no LeaveGuard.
 * - R2: each upload runs as a mutation on the app QueryClient, so a 401 takes the same `expired()` path as every request.
 * - R3: sign-out and another account's sign-in clear it (`clearUploads`, called from `clearLocalState`).
 * - R7: the XHR comes from an injectable factory (the default is the browser's); tests drive a fake one.
 *
 * `POST /api/agents/upload` (`agents/upload.rs` `upload_and_deploy`): multipart `name`, `file`, optional `version_tag`,
 * `ports` (CSV), `env` (a JSON object as a string), `writable` ("true"/"false"), `writable_path`, `inbound_format`;
 * 202 `{data: {build_id, agent_id, …}}`; errors are plain text (400/409/413/500).
 */
import type { QueryClient } from '@tanstack/react-query'
import { ApiError, isAbortError, safeJson } from '@/lib/api/client'
import type { components } from '@/lib/api/schema.gen'
import { clearFollower, follow, importFinished } from './follower'
import { parseReference } from './registry'

export interface UploadFields {
  name: string
  version?: string
  ports?: string
  env?: Record<string, string>
  /** Sent only when the user touched storage: the server treats these as tri-state. */
  writable?: boolean
  writablePath?: string
  inboundFormat?: 'openai' | 'anthropic' | 'gemini'
}

export type UploadState =
  | { phase: 'uploading'; fileName: string; fields: UploadFields; loaded: number; total: number }
  | { phase: 'done'; fileName: string; fields: UploadFields; buildId: string; agentId: string }
  | { phase: 'failed'; fileName: string; fields: UploadFields; error: ApiError }
  | { phase: 'cancelled'; fileName: string; fields: UploadFields }

export type XhrFactory = () => XMLHttpRequest

export const UPLOAD_PATH = '/api/agents/upload'

interface Entry {
  state: UploadState
  xhr: XMLHttpRequest | null
}

const entries = new Map<string, Entry>()
const listeners = new Set<() => void>()
let xhrFactory: XhrFactory = () => new XMLHttpRequest()

/** Tests: a fake XHR (R7). Returns the previous factory. */
export function setXhrFactory(factory: XhrFactory): XhrFactory {
  const prev = xhrFactory
  xhrFactory = factory
  return prev
}

function emit() {
  syncUnloadGuard()
  for (const l of listeners) l()
}

export function subscribeUploads(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}

export function uploadFor(userId: string): UploadState | null {
  return entries.get(userId)?.state ?? null
}

// ── beforeunload while any upload runs (R1) ──────────────────────────────────────────────
const onBeforeUnload = (e: BeforeUnloadEvent) => {
  e.preventDefault()
  // Older browsers need a returnValue to show the prompt.
  e.returnValue = ''
}
let guarding = false
function syncUnloadGuard() {
  const any =
    [...entries.values()].some((e) => e.state.phase === 'uploading') ||
    [...imports.values()].some((i) => i.phase === 'importing')
  if (any === guarding || typeof window === 'undefined') return
  guarding = any
  if (any) window.addEventListener('beforeunload', onBeforeUnload)
  else window.removeEventListener('beforeunload', onBeforeUnload)
}

function formFor(file: Blob, fileName: string, f: UploadFields): FormData {
  const fd = new FormData()
  fd.set('name', f.name)
  if (f.version) fd.set('version_tag', f.version)
  if (f.ports) fd.set('ports', f.ports)
  if (f.env && Object.keys(f.env).length) fd.set('env', JSON.stringify(f.env))
  if (f.writable !== undefined) fd.set('writable', String(f.writable))
  if (f.writable && f.writablePath) fd.set('writable_path', f.writablePath)
  if (f.inboundFormat) fd.set('inbound_format', f.inboundFormat)
  fd.set('file', file, fileName)
  return fd
}

/** One XHR upload with progress; resolves with the 202 body's ids, rejects with an `ApiError` (or an AbortError). */
function send(
  entry: Entry,
  userId: string,
  file: Blob,
  fileName: string,
  fields: UploadFields,
): Promise<{ buildId: string; agentId: string }> {
  return new Promise((resolve, reject) => {
    const xhr = xhrFactory()
    entry.xhr = xhr
    xhr.open('POST', UPLOAD_PATH)
    xhr.withCredentials = true
    xhr.setRequestHeader('Accept', 'application/json')
    xhr.upload.onprogress = (e) => {
      if (entries.get(userId) !== entry || entry.state.phase !== 'uploading') return
      entry.state = {
        ...entry.state,
        loaded: e.loaded,
        total: e.lengthComputable ? e.total : file.size,
      }
      emit()
    }
    xhr.onload = () => {
      const body = safeJson(xhr.responseText ?? '')
      if (xhr.status >= 200 && xhr.status < 300) {
        const data = (body as { data?: { build_id?: unknown; agent_id?: unknown } } | null)?.data
        if (data && typeof data.build_id === 'string')
          return resolve({ buildId: data.build_id, agentId: String(data.agent_id ?? '') })
        return reject(
          new ApiError(
            xhr.status,
            body,
            UPLOAD_PATH,
            `POST ${UPLOAD_PATH} → unexpected response (no build id)`,
          ),
        )
      }
      reject(new ApiError(xhr.status, body, UPLOAD_PATH, `POST ${UPLOAD_PATH} → ${xhr.status}`))
    }
    xhr.onerror = () =>
      reject(new ApiError(502, null, UPLOAD_PATH, `POST ${UPLOAD_PATH} → network error`))
    xhr.onabort = () => reject(new DOMException('Aborted', 'AbortError'))
    xhr.send(formFor(file, fileName, fields))
  })
}

/**
 * Start an upload for `userId`. One at a time per user: a running upload must finish or be cancelled first.
 * Runs as a mutation on `qc` (R2): its MutationCache `onError` sends a 401 to /login?expired=true.
 */
export function startUpload(
  qc: QueryClient,
  userId: string,
  file: Blob,
  fileName: string,
  fields: UploadFields,
): Promise<UploadState> {
  const current = entries.get(userId)
  if (current?.state.phase === 'uploading') return Promise.resolve(current.state)
  const entry: Entry = {
    state: { phase: 'uploading', fileName, fields, loaded: 0, total: file.size },
    xhr: null,
  }
  entries.set(userId, entry)
  emit()
  const mutation = qc.getMutationCache().build(qc, {
    mutationKey: ['deploy', 'upload'],
    mutationFn: () => send(entry, userId, file, fileName, fields),
    retry: false,
    // The file never goes into a cache: nothing to keep once it settles.
    gcTime: 0,
  })
  return mutation.execute(undefined).then(
    (ids) => {
      // Followed in the background until it finishes (design review 8).
      if (entries.get(userId) === entry) follow(qc, ids.buildId, fields.name, ids.agentId || null)
      return settle(userId, entry, { phase: 'done', fileName, fields, ...ids })
    },
    (err: unknown) => {
      if (isAbortError(err)) return settle(userId, entry, { phase: 'cancelled', fileName, fields })
      const error = err instanceof ApiError ? err : new ApiError(0, null, UPLOAD_PATH, String(err))
      return settle(userId, entry, { phase: 'failed', fileName, fields, error })
    },
  )
}

function settle(userId: string, entry: Entry, settled: UploadState): UploadState {
  // Env values may be secrets: once the upload is over they aren't kept in memory (the form still has its own copy).
  const state = { ...settled, fields: { ...settled.fields, env: undefined } }
  entry.xhr = null
  // A cleared or replaced entry (sign-out, another upload) is not written back.
  if (entries.get(userId) === entry) {
    entry.state = state
    emit()
  }
  return state
}

/** Cancel: aborts the XHR (the server has no cancel once the build is queued, D-2). */
export function cancelUpload(userId: string) {
  entries.get(userId)?.xhr?.abort()
}

/** Forget a finished upload (the form resets). */
export function dismissUpload(userId: string) {
  const e = entries.get(userId)
  if (e && e.state.phase !== 'uploading') {
    entries.delete(userId)
    emit()
  }
}

/** Sign-out and another account's sign-in (R3): abort everything and forget it (the server keeps importing). */
export function clearUploads() {
  const all = [...entries.values()]
  entries.clear()
  for (const e of all) e.xhr?.abort()
  imports.clear()
  clonedRepos.clear()
  epoch += 1
  clearFollower()
  emit()
}

// ── Registry imports (plans/feat-deploy.md §4.3; eng review R4) ─────────────────────────────────
//
// `POST /api/import/registry` pulls or builds and deploys inside one request (up to minutes) and reports nothing until
// it ends (D-7). It runs here like an upload: in-app navigation keeps it going, `beforeunload` guards tab close, it's a
// mutation on the app QueryClient (a 401 takes `expired()`), and sign-out forgets it. There is no cancel.

export const IMPORT_PATH = '/api/import/registry'

export type ImportState =
  | { phase: 'importing'; reference: string; startedAt: number }
  | {
      phase: 'done'
      reference: string
      agentId: string
      buildId: string | null
      containerName: string | null
    }
  | { phase: 'failed'; reference: string; error: ApiError }

const imports = new Map<string, ImportState>()

export function importFor(userId: string): ImportState | null {
  return imports.get(userId) ?? null
}

/** `ImportResult` (`catalog/import.rs`): 201 `{agent_id, build_id?, container_name?, status}`. */
type ImportResult = components['schemas']['ImportResult']

export function startImport(
  qc: QueryClient,
  userId: string,
  reference: string,
  send: (reference: string) => Promise<ImportResult>,
): Promise<ImportState> {
  const current = imports.get(userId)
  if (current?.phase === 'importing') return Promise.resolve(current)
  const started: ImportState = { phase: 'importing', reference, startedAt: Date.now() }
  imports.set(userId, started)
  emit()
  const mutation = qc.getMutationCache().build(qc, {
    mutationKey: ['deploy', 'import'],
    mutationFn: () => send(reference),
    retry: false,
    gcTime: 0,
  })
  const finish = (state: ImportState) => {
    // Cleared (sign-out) or replaced meanwhile: not written back.
    if (imports.get(userId) === started) {
      imports.set(userId, state)
      emit()
      if (state.phase === 'done') {
        const name = parseReference(reference)?.repo.split('/').pop() ?? reference
        const toasted = importFinished(
          state.containerName
            ? { kind: 'running', buildId: state.buildId, agentId: state.agentId, name }
            : { kind: 'notRunning', agentId: state.agentId, name },
        )
        // Announced by the toast: forgotten, so opening the Registry tab later doesn't jump to this old result.
        if (toasted) {
          imports.delete(userId)
          emit()
        }
      }
    }
    return state
  }
  return mutation.execute(undefined).then(
    (r) =>
      finish({
        phase: 'done',
        reference,
        agentId: r.agent_id,
        buildId: r.build_id ?? null,
        containerName: r.container_name ?? null,
      }),
    (err: unknown) =>
      finish({
        phase: 'failed',
        reference,
        error: err instanceof ApiError ? err : new ApiError(0, null, IMPORT_PATH, String(err)),
      }),
  )
}

export function dismissImport(userId: string) {
  const i = imports.get(userId)
  if (i && i.phase !== 'importing') {
    imports.delete(userId)
    emit()
  }
}

// ── GitHub clones started here (review D1, server gap D-12) ───────────────────────────────────
//
// `github_clone` writes no `github_url` (and nothing writes `commit_hash`), so a build record can't say which repository
// it came from. For clones started in this tab the repository is remembered, so the Build page's retry goes back to
// the GitHub tab with it picked. After a reload, or for someone else's build, the page falls back to Upload.

const clonedRepos = new Map<string, string>()

/** Bumped by every clear (sign-out, another account): work started before it must not write back after it. */
let epoch = 0
export const uploadsEpoch = () => epoch

export function rememberClone(buildId: string, repo: string) {
  clonedRepos.set(buildId, repo)
}

export const clonedRepo = (buildId: string): string | null => clonedRepos.get(buildId) ?? null
