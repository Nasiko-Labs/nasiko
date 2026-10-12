import type { Severity } from "../shared/types.js";

/** Client-side mirror of the server risk severity ladder. */
export function severityForScoreClient(score: number): Severity {
  if (score >= 75) return "critical";
  if (score >= 50) return "high";
  if (score >= 25) return "medium";
  return "low";
}
