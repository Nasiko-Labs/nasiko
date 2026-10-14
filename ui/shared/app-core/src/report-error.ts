/**
 * Client error reporting seam. Phase 2 replaces the sink with the
 * provider-neutral telemetry adapter; the redaction rule is already here:
 * only digest, name and a bounded message hash leave the page.
 */
export interface RedactedError {
  readonly digest: string | undefined;
  readonly name: string;
  readonly messageHash: string;
}

export function redactError(error: Error & { digest?: string }): RedactedError {
  return { digest: error.digest, name: error.name, messageHash: fnv1a(error.message).toString(16) };
}

export function reportClientError(error: Error & { digest?: string }): void {
  const payload = redactError(error);
  if (typeof window !== "undefined" && process.env.NODE_ENV !== "production") {
    console.info("[nasiko:error]", payload);
  }
}

function fnv1a(s: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h;
}
