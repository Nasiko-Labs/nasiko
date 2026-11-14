/**
 * The composer's mic (docs/superpowers/specs/2026-11-13-chat-dictation-design.md): records, sends the audio to
 * `POST /api/transcribe` and hands the text back; it never sends a message. F8 or Alt+R toggles it, Esc discards a
 * recording, and it stops by itself at DICTATION_MAX_MS. The microphone is released on stop, discard, failure and
 * unmount. Composer renders it only where `MediaRecorder` exists.
 */
import { LoaderCircle, Mic, Square } from 'lucide-react'
import { useEffect, useEffectEvent, useRef, useState } from 'react'
import { IS_MAC } from '@nasiko/ui/lib/keys'
import { Button } from '@nasiko/ui/primitives/button'
import { announce } from '../announce'
import { useTranscribe } from '../api'
import { copy, type ErrorKey } from '../copy'
import {
  clock,
  dictationErrorText,
  micErrorKey,
  recordingFileName,
  transcribeErrorKey,
} from '../dictation'
import { tuning } from '../tuning'

type Phase = 'idle' | 'starting' | 'recording' | 'transcribing'

interface Take {
  recorder: MediaRecorder
  stream: MediaStream
  discard: boolean
  cap: ReturnType<typeof setTimeout>
}

const release = (stream: MediaStream) => stream.getTracks().forEach((t) => t.stop())
const hint = copy.dictationHint(IS_MAC ? '⌥R' : 'Alt+R')

export function DictateButton({
  onText,
  onError,
}: {
  /** The transcript, trimmed and never empty. */
  onText(text: string): void
  /** A failure to show under the composer, or null to clear it. */
  onError(key: ErrorKey | null): void
}) {
  const transcription = useTranscribe()
  const [phase, setPhase] = useState<Phase>('idle')
  const [startedAt, setStartedAt] = useState(0)
  const [now, setNow] = useState(0)
  const take = useRef<Take | null>(null)
  const alive = useRef(false)
  // The recorder's callbacks outlive the render that started it: they read the composer's latest handlers.
  const latest = useRef({ onText, onError })
  useEffect(() => {
    latest.current = { onText, onError }
  })

  const fail = (key: ErrorKey) => {
    latest.current.onError(key)
    announce(dictationErrorText(key))
  }

  const end = (discard: boolean) => {
    const t = take.current
    if (!t) return
    t.discard = discard
    t.recorder.stop()
  }

  const start = async () => {
    latest.current.onError(null)
    if (!window.isSecureContext || !navigator.mediaDevices?.getUserMedia) return fail('micInsecure')
    setPhase('starting')
    let stream: MediaStream
    let recorder: MediaRecorder
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true })
    } catch (err) {
      setPhase('idle')
      return fail(micErrorKey(err))
    }
    if (!alive.current) return release(stream)
    try {
      recorder = new MediaRecorder(stream, { audioBitsPerSecond: tuning.DICTATION_BITS_PER_SECOND })
    } catch (err) {
      release(stream)
      setPhase('idle')
      return fail(micErrorKey(err))
    }
    const chunks: Blob[] = []
    recorder.ondataavailable = (e) => {
      if (e.data.size) chunks.push(e.data)
    }
    recorder.onstop = () => {
      const t = take.current
      take.current = null
      if (t) clearTimeout(t.cap)
      release(stream)
      if (!t || t.discard) {
        setPhase('idle')
        if (t && alive.current) announce(copy.dictationDiscarded)
        return
      }
      const type = recorder.mimeType || chunks[0]?.type || 'audio/webm'
      setPhase('transcribing')
      // Per-call callbacks don't fire after unmount, so a composer that's gone gets no text.
      transcription.mutate(
        { audio: new Blob(chunks, { type }), name: recordingFileName(type) },
        {
          onSuccess: (text) => {
            if (!text) return announce(copy.dictationEmpty)
            latest.current.onText(text)
            announce(copy.dictationInserted)
          },
          onError: (err) => fail(transcribeErrorKey(err)),
          onSettled: () => setPhase('idle'),
        },
      )
    }
    try {
      recorder.start()
    } catch (err) {
      // The browser can't record this stream: free the microphone and leave the button usable.
      release(stream)
      setPhase('idle')
      return fail(micErrorKey(err))
    }
    take.current = {
      recorder,
      stream,
      discard: false,
      cap: setTimeout(() => end(false), tuning.DICTATION_MAX_MS),
    }
    const t0 = Date.now()
    setStartedAt(t0)
    setNow(t0)
    setPhase('recording')
    announce(copy.dictationListening)
  }

  const toggle = () => {
    if (phase === 'idle') void start()
    else if (phase === 'recording') end(false)
  }

  useEffect(() => {
    alive.current = true
    return () => {
      alive.current = false
      const t = take.current
      if (!t) return
      t.discard = true
      release(t.stream)
      if (t.recorder.state !== 'inactive') t.recorder.stop()
    }
  }, [])

  useEffect(() => {
    if (phase !== 'recording') return
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [phase])

  const onKey = useEffectEvent((e: KeyboardEvent) => {
    // An open dialog, sheet or menu owns the keyboard (the same check as the composer's `/`).
    if (document.querySelector('[role=dialog][data-open], [role=alertdialog], [role=menu]')) return
    if (e.key === 'Escape') {
      if (phase !== 'recording') return
      e.preventDefault()
      return end(true)
    }
    // `code`, not `key`: on a Mac, Option+R types "®".
    const isToggle = e.key === 'F8' || (e.altKey && !e.ctrlKey && !e.metaKey && e.code === 'KeyR')
    if (!isToggle || e.repeat) return
    e.preventDefault()
    toggle()
  })
  useEffect(() => {
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  if (phase === 'recording')
    return (
      <Button
        type="button"
        size="sm"
        variant="outline"
        className="shrink-0 gap-1.5 px-2 text-destructive hover:text-destructive pointer-coarse:min-h-11"
        aria-label={copy.stopDictation}
        title={hint}
        onClick={() => end(false)}
      >
        <Square aria-hidden className="fill-current" />
        <span aria-hidden className="text-xs tabular-nums">
          {clock(now - startedAt)}
        </span>
      </Button>
    )

  const busy = phase !== 'idle'
  return (
    <Button
      type="button"
      size="icon-sm"
      variant="ghost"
      className="shrink-0 pointer-coarse:size-11"
      disabled={busy}
      aria-label={phase === 'transcribing' ? copy.transcribing : copy.startDictation}
      title={hint}
      onClick={toggle}
    >
      {phase === 'transcribing' ? (
        <LoaderCircle aria-hidden className="animate-spin motion-reduce:animate-none" />
      ) : (
        <Mic aria-hidden />
      )}
    </Button>
  )
}
