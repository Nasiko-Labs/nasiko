// Ported from nasiko-ui-lab@1b99bdda common/src/lib/draftKeys.ts. Local edits: a named eslint-disable on the storage presence check (this repo's lint rules).
/**
 * The chat drafts' storage prefix. Import-free so the app shell can clear drafts on sign out
 * without pulling the chat feature into the entry chunk (see app/shell/signOut.ts).
 */
export const DRAFT_PREFIX = "ui-lab:chat-draft:";

/** Remove every stored draft (all users) straight from localStorage; storage blocked: nothing to do. */
export function clearStoredDrafts() {
  try {
    const ls = globalThis.localStorage;
    // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- absent in Node, whatever lib.dom says
    if (!ls) return;
    for (let i = ls.length - 1; i >= 0; i--) {
      const k = ls.key(i);
      if (k?.startsWith(DRAFT_PREFIX)) ls.removeItem(k);
    }
  } catch {
    // Nothing to clear.
  }
}
