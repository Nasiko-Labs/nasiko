// Ported from nasiko-ui-lab@1b99bdda common/src/features/overview/health.ts (the `Rating` and `SpikeHit` types only).
// Local edits: split out of ./health.ts, which re-exports them, so ./copy.ts can name them without the copy ↔ health
// import cycle the architecture check rejects; Prettier formatting.
export type Rating = "healthy" | "watch" | "action" | "unknown";

export interface SpikeHit {
  date: string;
  factor: number;
}
