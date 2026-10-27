/**
 * The only live region in Chat (plan §8.1): announces turn outcomes, pre-turn send errors and
 * one-off failures (announce.ts), never the streaming text. The transcript stays a plain region.
 *
 * The spoken text is one piece of state that changes only when something new happens, so an
 * error clearing never makes the region fall back to, and re-read, an older message.
 */
import { useState } from 'react'
import { useAnnouncement } from '../announce'
import { announcement } from '../format'
import type { LiveTurn } from '../turnRegistry'

/** Screen readers skip a text node that didn't change: flip a trailing no-break space so a repeat is still spoken. */
const NBSP = '\u00a0'
const respeak = (prev: string, next: string) =>
  next === prev.replace(/\u00a0$/, '') && !prev.endsWith(NBSP) ? `${next}${NBSP}` : next

/** `error`: a failure before any turn exists (too long, busy, another tab), which has no phase to announce. */
export function StatusAnnouncer({
  live,
  agentName,
  askedBy,
  error = null,
}: {
  live: LiveTurn | undefined
  agentName: string
  askedBy?: string
  error?: string | null
}) {
  const oneOff = useAnnouncement()
  const [seen, setSeen] = useState({ live, error, seq: oneOff.seq, text: '' })
  if (seen.live !== live || seen.error !== error || seen.seq !== oneOff.seq) {
    // Everything new in this render, so a one-off landing with a turn outcome doesn't hide it.
    const parts = [
      oneOff.seq !== seen.seq ? oneOff.text : null,
      error && error !== seen.error ? error : null,
      live !== seen.live ? announcement(seen.live, live, agentName, askedBy) : null,
    ].filter((p): p is string => !!p)
    const next = parts.length ? parts.join('. ') : null
    setSeen({
      live,
      error,
      seq: oneOff.seq,
      text: next === null ? seen.text : respeak(seen.text, next),
    })
  }
  return (
    <div role="status" className="sr-only">
      {seen.text}
    </div>
  )
}

/**
 * One-off announcements inside a modal (dialog, sheet). A modal hides the rest of the page from
 * assistive tech, the page's StatusAnnouncer included, so while it is open this one speaks instead.
 */
export function ModalAnnouncer() {
  const oneOff = useAnnouncement()
  const [seen, setSeen] = useState({ seq: oneOff.seq, text: '' })
  if (seen.seq !== oneOff.seq) setSeen({ seq: oneOff.seq, text: respeak(seen.text, oneOff.text) })
  return (
    <div role="status" className="sr-only">
      {seen.text}
    </div>
  )
}
