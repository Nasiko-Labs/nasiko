// No lab counterpart as a module: the lab's Waiting row put the request in TanStack Router history state
// (`<Link state={{ waitingRequest }}>` in components/WaitingList.tsx, declared on `HistoryState` in search.ts) and
// SessionView read it, with the entry's key, through `useRouterState`. Local edits: n/a (new file). the navigation port's links take
// no state, so the row hands the request to the chat it opens here, and that chat stamps it into its own history entry
// on arrival (`waitingRequest`, as the lab named it, beside a `waitingNav` key unique to the click): like the lab's, it
// lives in that entry only (a crafted link can't carry it; Back/Forward to the entry restores it). The agents
// module's `deletedNote.ts` is the same pattern.
"use client";

import { useEffect, useSyncExternalStore } from "react";
import { historyIndex } from "@nasiko/app-core/url-state";

const REQUEST = "waitingRequest";
const NAV = "waitingNav";

let pending: { sessionId: string; requestId: string; key: string } | null = null;
let clicks = 0;
const listeners = new Set<() => void>();
const emit = () => {
  for (const l of listeners) l();
};

/** A Waiting row was clicked: the chat it opens focuses this request (or says it's gone). */
export function openFromWaiting(sessionId: string, requestId: string): void {
  pending = { sessionId, requestId, key: `waiting:${String(++clicks)}` };
  emit();
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  window.addEventListener("popstate", fn);
  return () => {
    listeners.delete(fn);
    window.removeEventListener("popstate", fn);
  };
}

/** One string per entry and request, so the snapshot is stable between changes. */
function read(sessionId: string, href: string): string {
  if (pending?.sessionId === sessionId) return `${pending.requestId}\n${pending.key}`;
  const state = (window.history.state as Record<string, unknown> | null) ?? {};
  const request = state[REQUEST];
  const nav = state[NAV];
  if (typeof request === "string" && typeof nav === "string") return `${request}\n${nav}`;
  // An entry not opened from Waiting: its position and URL tell one navigation from the next.
  return `\n${String(historyIndex())}|${href}`;
}

/**
 * The lab's `location.state.waitingRequest` and the entry's key for one chat (`href` is the URL on screen, so a
 * navigation inside the chat reads a new key). A request carried by a click is stamped into the entry on arrival.
 */
export function useWaitingNav(
  sessionId: string,
  href: string,
): { waitingRequest: string | undefined; navKey: string } {
  const snap = useSyncExternalStore(
    subscribe,
    () => read(sessionId, href),
    () => `\n${href}`,
  );
  useEffect(() => {
    if (pending?.sessionId !== sessionId) return;
    const { requestId, key } = pending;
    pending = null;
    const state = (window.history.state as Record<string, unknown> | null) ?? {};
    window.history.replaceState({ ...state, [REQUEST]: requestId, [NAV]: key }, "");
    emit();
  }, [sessionId, href]);
  const [request = "", navKey = ""] = snap.split("\n");
  return { waitingRequest: request || undefined, navKey };
}
