"use client";

import { formatCostUsd } from "@/lib/format";
import type { AgentMetricsResponse } from "@/lib/types";

/**
 * Tier B — Tiny prompt-vs-completion split inside the "Estimated cost" card.
 * Renders nothing when neither half is available so the card stays clean.
 */
export function CostSplit({
  breakdown,
}: {
  breakdown: AgentMetricsResponse["source"] extends infer S
    ? S extends { cost_breakdown?: infer B }
      ? B
      : never
    : never;
}) {
  if (!breakdown) {
    return null;
  }
  const prompt = breakdown.prompt_usd ?? 0;
  const completion = breakdown.completion_usd ?? 0;
  const total = prompt + completion;
  if (total <= 0) {
    return null;
  }
  const promptPct = (prompt / total) * 100;
  const completionPct = (completion / total) * 100;

  return (
    <div className="cost-split" role="img" aria-label="Cost split: prompt vs completion">
      <div className="cost-split__track" aria-hidden="true">
        <span
          className="cost-split__seg cost-split__seg--prompt"
          style={{ flex: `${promptPct.toFixed(2)} 0 0` }}
          title={`Prompt · ${formatCostUsd(prompt)} (${promptPct.toFixed(0)}%)`}
        />
        <span
          className="cost-split__seg cost-split__seg--completion"
          style={{ flex: `${completionPct.toFixed(2)} 0 0` }}
          title={`Completion · ${formatCostUsd(completion)} (${completionPct.toFixed(0)}%)`}
        />
      </div>
      <div className="cost-split__legend">
        <span>
          <span className="legend-swatch cost-split__swatch--prompt" aria-hidden /> Prompt {promptPct.toFixed(0)}%
        </span>
        <span>
          <span className="legend-swatch cost-split__swatch--completion" aria-hidden /> Completion {completionPct.toFixed(0)}%
        </span>
      </div>
    </div>
  );
}
