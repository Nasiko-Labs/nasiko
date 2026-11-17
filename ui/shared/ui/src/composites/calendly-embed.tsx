/**
 * A Calendly event embedded inline, as nasiko.com/demo does: a plain iframe (no Calendly script, so the app's
 * script policy stays hash-pinned; each server's CSP allows `frame-src https://calendly.com`). Used by Contact sales
 * and the free TokenOps assessment. Calendly posts
 * the embedding page a `calendly.event_scheduled` message when the person books, and `calendly.page_height` as
 * its content grows, which this listens for from Calendly's origin only.
 */
import { useEffect, useState } from 'react'
import { cn } from '@nasiko/ui/lib/utils'

const CALENDLY_ORIGIN = 'https://calendly.com'
const SCHEDULED = 'calendly.event_scheduled'
const PAGE_HEIGHT = 'calendly.page_height'
const VIEWED = 'calendly.event_type_viewed'
/** Tall enough for the date and time pickers before Calendly reports its own height. */
const MIN_HEIGHT_PX = 700
/** Shows whatever Calendly has when it never says the calendar is up, so the booking form is never left hidden. */
const REVEAL_AFTER_MS = 10_000

/** What Calendly reports when the person books: the scheduled event and the invitee, as API URIs. */
export interface CalendlyBooking {
  event_uri: string
  invitee_uri: string
}

type CalendlyMessage = { event?: unknown; payload?: Record<string, unknown> }

/** The Calendly message's data, when it is one; anything else is null. */
function calendlyData(e: MessageEvent): CalendlyMessage | null {
  if (e.origin !== CALENDLY_ORIGIN || typeof e.data !== 'object' || e.data === null) return null
  return e.data as CalendlyMessage
}

const uriOf = (v: unknown) =>
  typeof v === 'object' && v !== null && typeof (v as { uri?: unknown }).uri === 'string'
    ? (v as { uri: string }).uri
    : null

export function CalendlyEmbed({
  src,
  title,
  loadingLabel,
  onScheduled,
}: {
  src: string
  title: string
  loadingLabel: string
  onScheduled(booking: CalendlyBooking): void
}) {
  const [height, setHeight] = useState(MIN_HEIGHT_PX)
  // As nasiko.com/demo does (nasiko-website site/pages/demo.js `showWhenReady`): Calendly draws its loader at the top of a
  // still-empty page, so until it says the calendar is up (`calendly.event_type_viewed`, ~3 s) its frame is transparent
  // and our spinner sits in the middle; after REVEAL_AFTER_MS without it, whatever Calendly has is shown. Opacity, not
  // visibility: Chrome throttles a hidden cross-origin frame, which then never gets as far as that message.
  const [ready, setReady] = useState(false)
  useEffect(() => {
    const timer = window.setTimeout(() => setReady(true), REVEAL_AFTER_MS)
    return () => window.clearTimeout(timer)
  }, [])
  useEffect(() => {
    const onMessage = (e: MessageEvent) => {
      const data = calendlyData(e)
      if (!data) return
      if (data.event === VIEWED) {
        setReady(true)
        return
      }
      if (data.event === PAGE_HEIGHT) {
        const px = Number.parseInt(String(data.payload?.height ?? ''), 10)
        if (px > 0) setHeight(Math.max(MIN_HEIGHT_PX, px))
        return
      }
      if (data.event !== SCHEDULED) return
      const event_uri = uriOf(data.payload?.event)
      const invitee_uri = uriOf(data.payload?.invitee)
      if (event_uri && invitee_uri) onScheduled({ event_uri, invitee_uri })
    }
    window.addEventListener('message', onMessage)
    return () => window.removeEventListener('message', onMessage)
  }, [onScheduled])
  return (
    <div className="relative" style={{ height }} aria-busy={!ready}>
      {ready ? null : (
        // nasiko.com/demo's spinner (site.css `.bk__frame[aria-busy]::after`), slowed rather than stopped under reduced motion.
        <div
          role="img"
          aria-label={loadingLabel}
          className="absolute inset-0 m-auto size-7 animate-spin rounded-full border-2 border-border border-t-foreground [animation-duration:0.8s] motion-reduce:[animation-duration:2.4s]"
        />
      )}
      <iframe
        src={src}
        title={title}
        className={cn(
          'size-full rounded-lg border-0 transition-opacity duration-300',
          !ready && 'opacity-0',
        )}
        referrerPolicy="strict-origin-when-cross-origin"
      />
    </div>
  )
}
