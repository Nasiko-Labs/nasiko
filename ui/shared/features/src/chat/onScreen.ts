/**
 * "Has this element been on screen?" through one shared IntersectionObserver (v1b E-A6): a long
 * routed transcript observes every saved reply with a single observer, and each reply's flows
 * fetch starts only once it has been seen. Without IntersectionObserver, everything counts as seen.
 */
import { useEffect, useState, type RefObject } from 'react'

type Watch = (el: Element, onSeen: () => void) => () => void

let shared: { ctor: typeof IntersectionObserver; watch: Watch } | null = null

function watcher(): Watch | null {
  const Ctor = globalThis.IntersectionObserver
  if (!Ctor) return null
  // A test that swaps the global gets a fresh observer.
  if (shared?.ctor === Ctor) return shared.watch
  const callbacks = new Map<Element, () => void>()
  const observer = new Ctor((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue
      const cb = callbacks.get(e.target)
      if (!cb) continue
      callbacks.delete(e.target)
      observer.unobserve(e.target)
      cb()
    }
  })
  const watch: Watch = (el, onSeen) => {
    callbacks.set(el, onSeen)
    observer.observe(el)
    return () => {
      callbacks.delete(el)
      observer.unobserve(el)
    }
  }
  shared = { ctor: Ctor, watch }
  return watch
}

export function useSeen(ref: RefObject<Element | null>): boolean {
  const [seen, setSeen] = useState(() => !globalThis.IntersectionObserver)
  useEffect(() => {
    if (seen || !ref.current) return
    // No observer: the initial state already counted it as seen.
    return watcher()?.(ref.current, () => setSeen(true))
  }, [seen, ref])
  return seen
}
