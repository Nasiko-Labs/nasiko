import * as z from "zod";

/**
 * Feature flags are server-read (`NASIKO_FLAGS`), parsed once per request and
 * passed down as data. Unknown keys are rejected: a typo must not silently
 * disable a route. Every flag defaults to off.
 */
export const FlagsSchema = z
  .object({
    /** Exposes the private verification harness routes. Never on in production builds. */
    harness: z.boolean().default(false),
    /** Enables the Weave generated-surface runtime routes (Phase 4). */
    weaveSurface: z.boolean().default(false),
  })
  .strict();
export type Flags = z.infer<typeof FlagsSchema>;

export const FLAG_NAMES = Object.keys(FlagsSchema.shape) as (keyof Flags)[];

/** Parses `NASIKO_FLAGS="harness,weaveSurface"` into a Flags object. */
export function parseFlagList(list: string | undefined): Flags {
  const names = (list ?? "")
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
  const unknown = names.filter((n) => !FLAG_NAMES.includes(n as keyof Flags));
  if (unknown.length > 0)
    throw new Error(`unknown flag(s): ${unknown.join(", ")} (known: ${FLAG_NAMES.join(", ")})`);
  const obj: Record<string, boolean> = {};
  for (const n of names) obj[n] = true;
  return FlagsSchema.parse(obj);
}
