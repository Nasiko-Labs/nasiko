// Ported from nasiko-ui-lab@1b99bdda common/src/lib/utils.ts (`uuid`, whose one consumer is the chat registry).
// Local edits: moved beside its consumer (the destination's `@nasiko/ui/cn` holds only `cn`); `b[6]`/`b[8]` read
// with `?? 0` (noUncheckedIndexedAccess; both indexes exist in the 16-byte array); Prettier formatting.

/**
 * A v4 UUID. `crypto.randomUUID` exists only in secure contexts (HTTPS or localhost), and OSS nasiko-server
 * also serves plain HTTP (TODOS "chat ids over plain HTTP"); `getRandomValues` works everywhere.
 */
export function uuid(): string {
  if (typeof crypto.randomUUID === "function") return crypto.randomUUID();
  const b = crypto.getRandomValues(new Uint8Array(16));
  b[6] = ((b[6] ?? 0) & 0x0f) | 0x40;
  b[8] = ((b[8] ?? 0) & 0x3f) | 0x80;
  const h = [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}
