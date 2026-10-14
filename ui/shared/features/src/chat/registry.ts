// Ported from nasiko-ui-lab@1b99bdda common/src/features/chat/registry.ts. Local edits: the registry also holds the
// product scope its reads and invalidations are keyed under: `chatRegistry(client, scope, userId)` (`useChatRegistry`
// reads `useProductScope()`) keeps the latest page's scope, so a workspace switch re-keys them without aborting the
// user's turns, and `invalidateChat(client, scope, sessionId)` takes it; `meQuery` is `meQueryOptions(scope)`; `uuid`
// comes from ./uuid.ts (the destination's `@nasiko/ui/cn` holds only `cn`); the hot-reload dispose reads
// `import.meta.hot` or webpack's `import.meta.webpackHot` (a Next-era fallback; Vite provides `hot`); Prettier formatting.
/**
 * The app's one turn registry, wired to the real API (plan §6.2). Turns outlive pages, so the
 * registry lives at module scope, keyed by the signed-in user; a user switch clears it.
 */
import { useQueryClient, type QueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { useStore } from "zustand";
import { createStore, type StoreApi } from "zustand/vanilla";
import type { Scope } from "@nasiko/data-access";
import { apiFetch, meQueryOptions, useProductScope, withQuery } from "@nasiko/data-access/product";
import { uuid } from "./uuid";
import { announce } from "./announce";
import { chatKeys, createSession, resetPendingBackoff, saveMessage, sessionRows } from "./api";
import { copy } from "./copy";
import { createSignals, type Signals, type SignalsSnapshot } from "./signals";
import { unwrapMessagesPage } from "./normalize";
import { clearHitlIndex, indexRequests } from "./pending";
import { clearSessionLookup } from "./sessionLookup";
import { clearAnswersSeen } from "./turnModel";
import { createTurnRegistry, type LiveTurn, type TurnRegistry } from "./turnRegistry";
import { clearWaitingDebug } from "./waiting";

let current: {
  userId: string;
  /** The product scope the registry's reads and invalidations are keyed under: the latest page's (a workspace
   *  switch moves it without stopping the user's turns). */
  scope: Scope;
  registry: TurnRegistry;
  signals: Signals;
  unwatch: () => void;
} | null = null;

/**
 * Warn before the tab closes while any chat has a running turn or an unsaved reply (§6.2).
 * App-wide, because turns keep running after the user leaves the chat page.
 */
function watchUnload(registry: TurnRegistry): () => void {
  if (typeof window === "undefined") return () => undefined;
  const onLeave = (e: BeforeUnloadEvent) => {
    if (registry.busy()) e.preventDefault();
  };
  window.addEventListener("beforeunload", onLeave);
  return () => {
    window.removeEventListener("beforeunload", onLeave);
  };
}

function build(
  client: QueryClient,
  userId: string,
  scopeOf: () => Scope,
): { registry: TurnRegistry; signals: Signals } {
  let signals: Signals | null = null;
  const registry = createTurnRegistry({
    userId,
    createSession,
    saveMessage,
    dispatch: (body, signal) =>
      // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- absent in Node, whatever lib.dom says
      fetch(new URL("/api/orchestrator/a2a", globalThis.location?.origin ?? "http://localhost"), {
        method: "POST",
        body: JSON.stringify(body),
        signal,
        credentials: "same-origin",
        headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
      }),
    async replyExists(sessionId, userMessageId, content) {
      const page = unwrapMessagesPage(
        await apiFetch(
          withQuery(`/api/chat/sessions/${encodeURIComponent(sessionId)}/messages`, { limit: 100 }),
        ),
      );
      const i = page.data.findIndex((m) => m.id === userMessageId);
      // The user row isn't on the newest page: we can't prove a reply follows it.
      if (i < 0) return false;
      return page.data.slice(i + 1).some((m) => m.role === "assistant" && m.content === content);
    },
    // Re-check the session: its 401 runs the app's login redirect (queryClient.ts).
    onUnauthorized: () =>
      void client.query({ ...meQueryOptions(scopeOf()), staleTime: 0 }).catch(() => undefined),
    newId: uuid,
    now: () => Date.now(),
    locks: typeof navigator !== "undefined" && "locks" in navigator ? navigator.locks : null,
    warn: (message, context) => {
      console.warn(`[chat] ${message}`, context);
    },
    // The one place a routed turn's queries are refreshed, even with no chat page mounted (NE-4).
    onTurnEnd: (end) => void invalidateChat(client, scopeOf(), end.sessionId),
    // A direct save settled: refresh the chat unless its page is open (the page refreshes itself, E4).
    onDirectSaved: (sessionId) => {
      if (signals?.openChat() !== sessionId) void invalidateChat(client, scopeOf(), sessionId);
    },
  });
  const titleOf = (sessionId: string) =>
    sessionRows(client.getQueryData(chatKeys.list(scopeOf()))).find(
      (r) => r.session_id === sessionId,
    )?.title || copy.untitledChat;
  // Created with the registry, so a turn that ends while no chat page is mounted still counts (v1c E2).
  signals = createSignals({
    registry,
    now: () => Date.now(),
    invalidate: (sessionId) => void invalidateChat(client, scopeOf(), sessionId),
    onBackgroundError: (sessionId) => {
      announce(copy.replyFailedIn(titleOf(sessionId)));
    },
  });
  // The Waiting queue (v1c §5.9): a live `hitl` frame tells the index where its request lives (EN10), and a new
  // pause refetches the pending list, even with no chat page mounted.
  const pauses = new Set<string>();
  registry.subscribe(() => {
    for (const t of registry.snapshot()) {
      const id = t.state.request?.id;
      if (!id) continue;
      indexRequests(userId, t.sessionId, [id]);
      if (!pauses.has(id)) {
        pauses.add(id);
        void client.invalidateQueries({ queryKey: chatKeys.pendingAll(scopeOf()) });
      }
    }
  });
  return { registry, signals };
}

/** The registry for this user; a different user starts fresh (clearChatRegistry on sign out; on another tab's sign-in as a
 * different account, that tab reloads, since a user switch alone doesn't stop turns until a chat page mounts). */
export function chatRegistry(client: QueryClient, scope: Scope, userId: string): TurnRegistry {
  if (current?.userId !== userId) {
    if (current) clearSessionLookup();
    current?.registry.clearAll();
    current?.unwatch();
    current?.signals.dispose();
    const scopeOf = () => current?.scope ?? scope;
    const { registry, signals } = build(client, userId, scopeOf);
    current = { userId, scope, registry, signals, unwatch: watchUnload(registry) };
  }
  current.scope = scope;
  return current.registry;
}

export function clearChatRegistry() {
  clearSessionLookup();
  clearHitlIndex();
  resetPendingBackoff();
  clearWaitingDebug();
  clearAnswersSeen();
  current?.registry.clearAll();
  // Timers, the Reply-ready clock and the title writer go with it (E2).
  current?.signals.dispose();
  current?.unwatch();
  current = null;
}

/** This user's background-turn signals (v1c §5.8); null before a chat page made the registry. */
export const chatSignals = (): Signals | null => current?.signals ?? null;

const NO_SIGNALS: Pick<
  StoreApi<SignalsSnapshot | null>,
  "getState" | "getInitialState" | "subscribe"
> = createStore<null>(() => null);
/** The signals' current snapshot (null before a chat page made the registry); re-renders when it changes. */
export function useSignals(): SignalsSnapshot | null {
  return useStore(current?.signals.store ?? NO_SIGNALS);
}

/** Subscribe a component to one chat's live turn (an immutable snapshot). */
export function useLiveTurn(
  registry: TurnRegistry,
  sessionId: string | undefined,
): LiveTurn | undefined {
  return useStore(registry.store, (s) => (sessionId ? s.turns.get(sessionId) : undefined));
}

/** The registry's routed ends (for `endOf`); a stable value until an end is recorded. */
export const useTurnEnds = (registry: TurnRegistry) => useStore(registry.store, (s) => s.ends);
/** Steps kept for saved replies (for `stepsOf`); a stable value until one is kept. */
export const useSavedSteps = (registry: TurnRegistry) => useStore(registry.store, (s) => s.steps);

export function useChatRegistry(userId: string): TurnRegistry {
  return chatRegistry(useQueryClient(), useProductScope(), userId);
}

/** Refresh the list and this chat's history after a turn changes the server state. */
export const invalidateChat = (client: QueryClient, scope: Scope, sessionId: string) =>
  Promise.all([
    client.invalidateQueries({ queryKey: chatKeys.history(scope, sessionId) }),
    client.invalidateQueries({ queryKey: chatKeys.list(scope) }),
  ]);

interface HotModule {
  dispose: (cb: () => void) => void;
}

/** Dev: a hot reload of the registry aborts its live turns; say so, since a routed reply may be lost (DX-4, NX-10). */
export function disposeForHotReload(
  warn: (message: string) => void = (m) => {
    console.warn(m);
  },
) {
  const live = current?.registry.liveCount() ?? 0;
  if (live)
    warn(
      `[chat] hot reload aborted ${live} live turn(s). A routed reply is saved server-side only if the stream reached Done; reopen the chat to check.`,
    );
  clearChatRegistry();
}

// Dev: a hot reload of this module must not leave the old registry's turns running. Vite names it `import.meta.hot`
// (`webpackHot` is a Next-era fallback, never set under Vite). Without it a re-executed module would start a fresh
// registry beside the old one's still-running turns (dev only).
const hot = import.meta as ImportMeta & { hot?: HotModule; webpackHot?: HotModule };
(hot.hot ?? hot.webpackHot)?.dispose(() => {
  disposeForHotReload();
});

/**
 * The hidden-tab count a chat page puts before its <title> (v1c C4): "(N) " while the tab is hidden and
 * N replies are unseen. While a chat page is mounted React owns the title; otherwise the signals write it.
 */
export function useTitlePrefix(): string {
  const s = current?.signals ?? null;
  const snap = useSignals();
  useEffect(() => s?.ownTitle(), [s]);
  return snap?.titlePrefix ?? "";
}
