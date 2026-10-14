/**
 * The scope and canonical-argument helpers every query key uses. Kept free of `@nasiko/contracts` values (and so of
 * zod): the product keys (`./product/keys`) import this module, and the signed-in app shell loads them on first
 * paint — importing `./keys` instead pulled every contract schema into the shell bundle (UI integration milestone,
 * check:bundles).
 */
export interface Scope {
  readonly workspaceId: string;
  readonly epoch: number;
}

export function canonicalArgs(args: unknown): unknown {
  if (Array.isArray(args)) return args.map(canonicalArgs);
  if (args && typeof args === "object") {
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(args).sort()) {
      const v = (args as Record<string, unknown>)[k];
      if (v !== undefined) out[k] = canonicalArgs(v);
    }
    return out;
  }
  return args;
}
