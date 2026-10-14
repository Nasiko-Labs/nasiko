// No lab counterpart (the lab's tsconfig loaded `vite/client`, which declares `?raw` imports). Local edits: n/a (new
// file). The lab's tests read their CLI `--help` fixtures as text with Vite's `?raw` suffix; vitest serves them, and
// this tells TypeScript what such an import is.
declare module "*?raw" {
  const text: string;
  export default text;
}

/** Vite's `import.meta.glob` (eager, `?raw`), which the agents copy test uses to scan its sources. */
interface ImportMeta {
  glob(
    patterns: string | readonly string[],
    options?: { query?: string; import?: string; eager?: boolean },
  ): Record<string, unknown>;
}
