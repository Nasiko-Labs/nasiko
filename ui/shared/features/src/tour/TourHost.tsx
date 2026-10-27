/**
 * Mounted on every signed-in page (`_app.tsx`): opens this page's tour when asked (the top bar's tour button, or `?`;
 * the app tour, the sidebar's, on a page without one, or from the account menu's Tour the sidebar), and on its own only on the Overview, once, for a first-time
 * user (one the onboarding endpoint called first-time in this browser) once the setup guide closes, finished or
 * skipped, and the page has loaded, so its targets are there. A tour belongs to its page: leaving the page closes it.
 */
import { useIsFetching, useQuery } from '@tanstack/react-query'
import { useRouterState } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { useSlots } from '@nasiko/app-core/edition/edition-context'
import { useGuide, useGuideState } from '@nasiko/features/onboarding/api'
import { meQuery } from '@nasiko/data-access/auth'
import { isTypingTarget } from '@nasiko/ui/lib/keys'
import { askForTour, useTourAsks } from '@nasiko/app-core/tour'
import { TourOverlay } from './index'
import { dueTour, markSeen, tourFor, type TourPrefs } from './logic'
import { readPrefs, useTourPrefs, writePrefs } from './store'

/** How long the page stays settled (no query in flight) before a tour opens on its own. */
const SETTLE_MS = 700

/** Where `?` is someone else's key: typing, a menu or list's typeahead, or a dialog (the tour's own included). */
const OWN_KEYS =
  '[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], [role="combobox"]'

interface Open {
  id: string
  path: string
  /** Opened on its own (the Overview's, for a first-time user): it offers No thanks. */
  firstRun: boolean
}

export function TourHost() {
  const pathname = useRouterState({ select: (s) => s.location.pathname })
  // The _app guard already loaded `me`; this only reads it.
  const sub = useQuery({ ...meQuery, refetchOnMount: false, retryOnMount: false }).data?.sub
  const prefs = useTourPrefs(sub)
  const guide = useGuide()
  const guideOpen = useGuideState().open
  const busy = useIsFetching() > 0
  const layerTours = useSlots().tours
  const [open, setOpen] = useState<Open | null>(null)

  // Asked for: this page's tour, or the app tour on a page without one.
  const asks = useTourAsks()
  const [answered, setAnswered] = useState(asks)
  if (asks !== answered) {
    setAnswered(asks)
    setOpen({
      id: asks.tour ?? tourFor(pathname, layerTours) ?? 'app',
      path: pathname,
      firstRun: false,
    })
  }
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== '?' || e.metaKey || e.ctrlKey || e.altKey || e.repeat || e.defaultPrevented)
        return
      if (isTypingTarget(e.target) || (e.target instanceof Element && e.target.closest(OWN_KEYS)))
        return
      e.preventDefault()
      askForTour()
    }
    addEventListener('keydown', onKey)
    return () => removeEventListener('keydown', onKey)
  }, [])
  if (open && open.path !== pathname) setOpen(null)

  // A first-time user turns the automatic tour on, once: after No thanks or the tour itself (which mark the Overview's
  // seen) it stays off.
  useEffect(() => {
    if (sub && guide.due && !prefs.auto && !prefs.seen.includes('overview'))
      writePrefs(sub, { ...prefs, auto: true })
  }, [sub, guide.due, prefs])

  const dueId = sub && !guide.due && !guideOpen && !open ? dueTour(prefs, pathname) : null
  useEffect(() => {
    if (!dueId || busy) return
    const t = setTimeout(() => setOpen({ id: dueId, path: pathname, firstRun: true }), SETTLE_MS)
    return () => clearTimeout(t)
  }, [dueId, busy, pathname])

  if (!open || !sub) return null
  const end = (next: (p: TourPrefs) => TourPrefs) => {
    writePrefs(sub, next(readPrefs(sub)))
    setOpen(null)
  }
  return (
    <TourOverlay
      key={`${open.id}:${open.path}`}
      id={open.id}
      layer={layerTours.find((t) => t.id === open.id) ?? null}
      // The one place to turn the automatic tour off.
      offerOptOut={open.firstRun}
      onEnd={() => end((p) => markSeen(p, open.id))}
      onOptOut={() => end((p) => ({ ...markSeen(p, open.id), auto: false }))}
    />
  )
}
