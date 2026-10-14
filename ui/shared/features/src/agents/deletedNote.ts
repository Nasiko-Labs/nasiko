// No lab counterpart as a module: the lab carried the delete result in TanStack Router history state
// (`navigate({ state: { agentDeleted } })` in components/dialogs.tsx, read with `useRouterState` in MyAgentsPage.tsx).
// Local edits: n/a (new file). the port's `router.replace` takes no state, so the note waits here for the page it was sent
// to, which stamps it into its own history entry (`agentDeleted`, as the lab named it) on arrival: like the lab's, it
// lives in that entry only (a crafted link can't show it; Back/Forward to the entry restores it), and dismissing it
// replaces the entry's state without it.
"use client";

import { useEffect, useSyncExternalStore } from "react";
import type { DeletedNote } from "./search";

const KEY = "agentDeleted";

let pending: { path: string; note: DeletedNote } | null = null;
const listeners = new Set<() => void>();
const emit = () => {
  for (const l of listeners) l();
};

/** The delete dialog: the note for the page it is about to replace the entry with (`path`, no query). */
export function carryDeletedNote(path: string, note: DeletedNote): void {
  pending = { path, note };
}

function isNote(v: unknown): v is DeletedNote {
  const n = v as Partial<DeletedNote> | null;
  return (
    !!n &&
    typeof n === "object" &&
    typeof n.name === "string" &&
    Array.isArray(n.errors) &&
    typeof n.stopped === "number"
  );
}

function read(): DeletedNote | null {
  if (typeof window === "undefined") return null;
  const v = (window.history.state as Record<string, unknown> | null)?.[KEY];
  return isNote(v) ? v : null;
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  window.addEventListener("popstate", fn);
  return () => {
    listeners.delete(fn);
    window.removeEventListener("popstate", fn);
  };
}

/**
 * The current entry's delete note (the lab's `location.state.agentDeleted`), stamping a carried one into the entry on
 * arrival at `path`, and `clear()` (replace the entry's state without it, the lab's dismiss).
 */
export function useDeletedNote(path: string): { note: DeletedNote | null; clear: () => void } {
  useEffect(() => {
    if (!pending || pending.path !== path) return;
    const { note } = pending;
    pending = null;
    const state = (window.history.state as Record<string, unknown> | null) ?? {};
    window.history.replaceState({ ...state, [KEY]: note }, "");
    emit();
  }, [path]);
  const note = useSyncExternalStore(subscribe, read, () => null);
  const clear = () => {
    const state = (window.history.state as Record<string, unknown> | null) ?? {};
    // eslint-disable-next-line @typescript-eslint/no-unused-vars -- the dropped note
    const { [KEY]: _, ...rest } = state;
    window.history.replaceState(rest, "");
    emit();
  };
  return { note, clear };
}
