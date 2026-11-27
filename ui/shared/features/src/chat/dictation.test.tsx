/**
 * Dictation (docs/superpowers/specs/2026-11-13-chat-dictation-design.md): the pure helpers, then the composer's mic
 * against a stubbed MediaRecorder and microphone, with `POST /api/transcribe` answered by the chat mock.
 */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@nasiko/data-access/client'
import { configureMocks } from '@nasiko/mocks/handlers'
import { recordRequests } from '@nasiko/testing/setup'
import { Composer } from './components/Composer'
import { errorCopy } from './copy'
import { clock, insertAt, micErrorKey, recordingFileName, transcribeErrorKey } from './dictation'
import { tuning } from './tuning'

describe('dictation helpers', () => {
  it('names the upload after what the browser recorded', () => {
    expect(recordingFileName('audio/webm;codecs=opus')).toBe('dictation.webm')
    expect(recordingFileName('audio/mp4')).toBe('dictation.mp4')
    expect(recordingFileName('audio/ogg;codecs=opus')).toBe('dictation.ogg')
    expect(recordingFileName('')).toBe('dictation.webm')
  })

  it('inserts at the cursor, replacing the selection, with a space only when needed', () => {
    expect(insertAt('', 0, 0, 'hi there')).toEqual({ value: 'hi there', caret: 8 })
    expect(insertAt('Say', 3, 3, 'hello')).toEqual({ value: 'Say hello', caret: 9 })
    expect(insertAt('Say ', 4, 4, 'hello')).toEqual({ value: 'Say hello', caret: 9 })
    expect(insertAt('Line\n', 5, 5, 'two')).toEqual({ value: 'Line\ntwo', caret: 8 })
    expect(insertAt('Say X now', 4, 5, 'hello')).toEqual({ value: 'Say hello now', caret: 9 })
    expect(insertAt('end', 0, 0, 'Start')).toEqual({ value: 'Startend', caret: 5 })
  })

  it('formats elapsed time as m:ss', () => {
    expect(clock(0)).toBe('0:00')
    expect(clock(12_400)).toBe('0:12')
    expect(clock(65_000)).toBe('1:05')
    expect(clock(300_000)).toBe('5:00')
  })

  it('maps microphone failures by their DOMException name', () => {
    const named = (name: string) => Object.assign(new Error(name), { name })
    expect(micErrorKey(named('NotAllowedError'))).toBe('micDenied')
    expect(micErrorKey(named('SecurityError'))).toBe('micDenied')
    expect(micErrorKey(named('NotFoundError'))).toBe('micMissing')
    expect(micErrorKey(new TypeError('boom'))).toBe('micFailed')
  })

  it('maps transcription failures by status', () => {
    const api = (status: number, body: unknown = 'x') =>
      new ApiError(status, body, '/api/transcribe', '')
    expect(transcribeErrorKey(api(503))).toBe('transcribeUnconfigured')
    expect(transcribeErrorKey(api(502, 'transcription API error'))).toBe('transcribeRejected')
    // apiFetch's own network failure is a 502 with no body: Open Runtime, not the provider.
    expect(transcribeErrorKey(api(502, null))).toBe('transcribeFailed')
    expect(transcribeErrorKey(api(400, 'failed to read file'))).toBe('transcribeUnreadable')
    expect(transcribeErrorKey(api(500))).toBe('transcribeFailed')
    expect(transcribeErrorKey(new TypeError('boom'))).toBe('transcribeFailed')
  })
})

// ─── The composer's mic ──────────────────────────────────────────────────────

const track = { stop: vi.fn() }
const stream = { getTracks: () => [track] }
let recorders: FakeRecorder[] = []

class FakeRecorder {
  state: 'inactive' | 'recording' = 'inactive'
  mimeType = 'audio/webm;codecs=opus'
  ondataavailable: ((e: { data: Blob }) => void) | null = null
  onstop: (() => void) | null = null
  readonly options?: { audioBitsPerSecond?: number }
  constructor(_stream: unknown, options?: { audioBitsPerSecond?: number }) {
    this.options = options
    recorders.push(this)
  }
  static failStart = false
  start() {
    if (FakeRecorder.failStart)
      throw Object.assign(new Error('cannot record'), { name: 'NotSupportedError' })
    this.state = 'recording'
  }
  stop() {
    this.state = 'inactive'
    this.ondataavailable?.({ data: new Blob(['audio'], { type: this.mimeType }) })
    this.onstop?.()
  }
}

const getUserMedia = vi.fn()

function setBrowser({ secure = true, recorder = true } = {}) {
  Object.defineProperty(window, 'isSecureContext', { configurable: true, value: secure })
  Object.defineProperty(navigator, 'mediaDevices', {
    configurable: true,
    value: secure ? { getUserMedia } : undefined,
  })
  if (recorder) vi.stubGlobal('MediaRecorder', FakeRecorder)
}

/** The composer with its draft held like the chat views hold it, so inserted text shows in the box. */
function Harness({ initial = '' }: { initial?: string }) {
  const [value, setValue] = useState(initial)
  return (
    <Composer
      agentName="ops"
      value={value}
      onChange={setValue}
      onSend={onSend}
      onStop={vi.fn()}
      onGoToRequest={vi.fn()}
      onNewChat={vi.fn()}
      mode={{ kind: 'ready' }}
    />
  )
}
const onSend = vi.fn()

function renderComposer(initial?: string) {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } })
  const { unmount } = render(
    <QueryClientProvider client={client}>
      <Harness initial={initial} />
    </QueryClientProvider>,
  )
  return Object.assign(screen.getByLabelText<HTMLTextAreaElement>('Message ops'), { unmount })
}

const transcribeCalls = (urls: URL[]) => urls.filter((u) => u.pathname === '/api/transcribe')
const fullError = (key: keyof typeof errorCopy) =>
  `${errorCopy[key].problem} ${errorCopy[key].cause} ${errorCopy[key].action}`

describe('Composer dictation', () => {
  beforeEach(() => {
    recorders = []
    FakeRecorder.failStart = false
    track.stop.mockClear()
    getUserMedia.mockReset().mockResolvedValue(stream)
  })
  afterEach(() => {
    vi.unstubAllGlobals()
    Reflect.deleteProperty(navigator, 'mediaDevices')
    Reflect.deleteProperty(window, 'isSecureContext')
    configureMocks({ chatVariants: [] })
    onSend.mockClear()
  })

  it('shows no mic where the browser cannot record', () => {
    setBrowser({ recorder: false })
    renderComposer()
    expect(screen.queryByRole('button', { name: 'Start dictation' })).toBeNull()
  })

  it('records, transcribes and inserts the text at the cursor without sending', async () => {
    setBrowser()
    const box = renderComposer('Please check')
    box.setSelectionRange(12, 12)
    const reqs = recordRequests()
    await userEvent.click(screen.getByRole('button', { name: 'Start dictation' }))
    expect(recorders[0]?.options?.audioBitsPerSecond).toBe(tuning.DICTATION_BITS_PER_SECOND)
    await userEvent.click(screen.getByRole('button', { name: /Stop dictation/ }))
    await waitFor(() => expect(box).toHaveValue('Please check Hello from the mock microphone.'))
    expect(box).toHaveFocus()
    expect(box.selectionStart).toBe(box.value.length)
    expect(transcribeCalls(reqs.urls)).toHaveLength(1)
    expect(track.stop).toHaveBeenCalled()
    expect(onSend).not.toHaveBeenCalled()
    reqs.stop()
  })

  it('keeps the draft and explains when transcription is not set up', async () => {
    setBrowser()
    configureMocks({ chatVariants: ['transcribe-unconfigured'] })
    const box = renderComposer('draft')
    await userEvent.click(screen.getByRole('button', { name: 'Start dictation' }))
    await userEvent.click(screen.getByRole('button', { name: /Stop dictation/ }))
    expect(await screen.findByText(fullError('transcribeUnconfigured'))).toBeInTheDocument()
    expect(box).toHaveValue('draft')
    expect(screen.getByRole('button', { name: 'Start dictation' })).toBeEnabled()
  })

  it('explains a denied microphone and sends nothing', async () => {
    setBrowser()
    getUserMedia.mockRejectedValue(Object.assign(new Error('denied'), { name: 'NotAllowedError' }))
    const reqs = recordRequests()
    renderComposer()
    await userEvent.click(screen.getByRole('button', { name: 'Start dictation' }))
    expect(await screen.findByText(fullError('micDenied'))).toBeInTheDocument()
    expect(transcribeCalls(reqs.urls)).toHaveLength(0)
    reqs.stop()
  })

  it('releases the microphone and recovers when the recorder cannot start', async () => {
    setBrowser()
    FakeRecorder.failStart = true
    const reqs = recordRequests()
    renderComposer()
    await userEvent.click(screen.getByRole('button', { name: 'Start dictation' }))
    expect(await screen.findByText(fullError('micFailed'))).toBeInTheDocument()
    expect(track.stop).toHaveBeenCalled()
    expect(screen.getByRole('button', { name: 'Start dictation' })).toBeEnabled()
    expect(transcribeCalls(reqs.urls)).toHaveLength(0)
    reqs.stop()
  })

  it('explains that the microphone needs HTTPS on an insecure page', async () => {
    setBrowser({ secure: false })
    renderComposer()
    await userEvent.click(screen.getByRole('button', { name: 'Start dictation' }))
    expect(await screen.findByText(fullError('micInsecure'))).toBeInTheDocument()
    expect(getUserMedia).not.toHaveBeenCalled()
  })

  it('Esc while recording discards the audio', async () => {
    setBrowser()
    const reqs = recordRequests()
    const box = renderComposer('draft')
    await userEvent.click(screen.getByRole('button', { name: 'Start dictation' }))
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(await screen.findByRole('button', { name: 'Start dictation' })).toBeInTheDocument()
    expect(track.stop).toHaveBeenCalled()
    expect(transcribeCalls(reqs.urls)).toHaveLength(0)
    expect(box).toHaveValue('draft')
    reqs.stop()
  })

  it('F8 and Alt+R start and stop dictation', async () => {
    setBrowser()
    const box = renderComposer()
    fireEvent.keyDown(window, { key: 'F8' })
    expect(await screen.findByRole('button', { name: /Stop dictation/ })).toBeInTheDocument()
    fireEvent.keyDown(window, { key: '®', code: 'KeyR', altKey: true })
    await waitFor(() => expect(box).toHaveValue('Hello from the mock microphone.'))
  })

  it('stops by itself at the length cap and transcribes', async () => {
    setBrowser()
    const cap = tuning.DICTATION_MAX_MS
    tuning.DICTATION_MAX_MS = 30
    try {
      const box = renderComposer()
      await userEvent.click(screen.getByRole('button', { name: 'Start dictation' }))
      await waitFor(() => expect(box).toHaveValue('Hello from the mock microphone.'))
      expect(recorders[0]?.state).toBe('inactive')
    } finally {
      tuning.DICTATION_MAX_MS = cap
    }
  })

  it('releases the microphone when the composer goes away mid-recording', async () => {
    setBrowser()
    const reqs = recordRequests()
    const box = renderComposer()
    await userEvent.click(screen.getByRole('button', { name: 'Start dictation' }))
    act(() => box.unmount())
    expect(track.stop).toHaveBeenCalled()
    expect(transcribeCalls(reqs.urls)).toHaveLength(0)
    reqs.stop()
  })
})
