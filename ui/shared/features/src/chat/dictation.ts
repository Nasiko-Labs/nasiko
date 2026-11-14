/**
 * Dictation's pure rules (docs/superpowers/specs/2026-11-13-chat-dictation-design.md); the recorder itself is
 * `components/DictateButton.tsx`.
 */
import { ApiError } from '@nasiko/data-access/client'
import { errorCopy, type ErrorKey } from './copy'

/** A dictation failure as one line under the composer (and in the live region). */
export const dictationErrorText = (key: ErrorKey) =>
  `${errorCopy[key].problem} ${errorCopy[key].cause} ${errorCopy[key].action}`

/** Whisper reads the format from the file name, so the upload is named after what the browser recorded. */
export function recordingFileName(mime: string): string {
  const ext = /^audio\/(mp4|ogg)\b/.exec(mime)?.[1] ?? 'webm'
  return `dictation.${ext}`
}

/** The transcript in place of the selection, with a space before it unless the text before ends in whitespace. */
export function insertAt(
  value: string,
  start: number,
  end: number,
  text: string,
): { value: string; caret: number } {
  const before = value.slice(0, start)
  const glued = before + (before && !/\s$/.test(before) ? ' ' : '') + text
  return { value: glued + value.slice(end), caret: glued.length }
}

/** Elapsed recording time as `m:ss`. */
export const clock = (ms: number): string => {
  const s = Math.floor(ms / 1000)
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`
}

/** `getUserMedia` / `MediaRecorder` failures: the DOMException name is the only signal browsers agree on. */
export function micErrorKey(err: unknown): ErrorKey {
  const name = err instanceof Error ? err.name : ''
  if (name === 'NotAllowedError' || name === 'SecurityError') return 'micDenied'
  if (name === 'NotFoundError') return 'micMissing'
  return 'micFailed'
}

/** `POST /api/transcribe` failures (`oss/server/src/transcribe.rs`). */
export function transcribeErrorKey(err: unknown): ErrorKey {
  if (!(err instanceof ApiError)) return 'transcribeFailed'
  if (err.status === 503) return 'transcribeUnconfigured'
  // quirk: apiFetch reports a network failure as a 502 with no body; the server's own 502s carry text.
  if (err.status === 502 && err.body !== null) return 'transcribeRejected'
  if (err.status === 400) return 'transcribeUnreadable'
  return 'transcribeFailed'
}
