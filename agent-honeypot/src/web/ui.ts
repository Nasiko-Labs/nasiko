import type { Outcome, Severity } from "../shared/types.js";

export const SEV_COLOR: Record<Severity, string> = {
  low: "text-low",
  medium: "text-medium",
  high: "text-high",
  critical: "text-critical",
};

export const SEV_BG: Record<Severity, string> = {
  low: "bg-low/15 border-low/40 text-low",
  medium: "bg-medium/15 border-medium/40 text-medium",
  high: "bg-high/15 border-high/40 text-high",
  critical: "bg-critical/15 border-critical/40 text-critical",
};

export function outcomeIcon(o: Outcome): string {
  switch (o) {
    case "executed":
    case "approved":
      return "✓";
    case "blocked":
    case "rejected":
      return "✕";
    case "ask":
      return "⚠";
  }
}

export function outcomeColor(o: Outcome): string {
  switch (o) {
    case "executed":
      return "text-low";
    case "approved":
      return "text-accent";
    case "blocked":
    case "rejected":
      return "text-critical";
    case "ask":
      return "text-medium";
  }
}

export function outcomeLabel(o: Outcome): string {
  return {
    executed: "EXECUTED",
    approved: "APPROVED",
    blocked: "BLOCKED",
    rejected: "REJECTED",
    ask: "ASK",
  }[o];
}

export function fmtTime(iso: string): string {
  return new Date(iso).toLocaleTimeString("en-GB", { hour12: false });
}
