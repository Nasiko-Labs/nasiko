/**
 * This browser's tour prefs per user, in localStorage (every tab agrees; the server keeps no tour state). Snapshots
 * are cached by raw string, so `useSyncExternalStore` sees a stable object until the value changes.
 */
import { useSyncExternalStore } from 'react'
import { NO_PREFS, parsePrefs, prefsKey, type TourPrefs } from './logic'

const listeners = new Set<() => void>()
function subscribe(fn: () => void) {
  listeners.add(fn)
  globalThis.addEventListener?.('storage', fn)
  return () => {
    listeners.delete(fn)
    globalThis.removeEventListener?.('storage', fn)
  }
}

/** Storage blocked: the prefs last while this page is open. */
const memory = new Map<string, string>()
function readRaw(sub: string): string | null {
  try {
    return localStorage.getItem(prefsKey(sub)) ?? memory.get(sub) ?? null
  } catch {
    return memory.get(sub) ?? null
  }
}

let cache: { raw: string | null; prefs: TourPrefs } = { raw: null, prefs: NO_PREFS }
function snapshot(sub: string | undefined): TourPrefs {
  if (!sub) return NO_PREFS
  const raw = readRaw(sub)
  if (raw !== cache.raw) cache = { raw, prefs: parsePrefs(raw) }
  return cache.prefs
}

export function writePrefs(sub: string, prefs: TourPrefs) {
  const raw = JSON.stringify(prefs)
  try {
    localStorage.setItem(prefsKey(sub), raw)
  } catch {
    memory.set(sub, raw)
  }
  for (const fn of listeners) fn()
}

export function readPrefs(sub: string): TourPrefs {
  return snapshot(sub)
}

export const useTourPrefs = (sub: string | undefined) =>
  useSyncExternalStore(
    subscribe,
    () => snapshot(sub),
    () => NO_PREFS,
  )
