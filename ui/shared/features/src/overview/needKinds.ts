// Split out of ./needs.ts (nasiko-ui-lab@1b99bdda common/src/features/overview/needs.ts) so ./copy.ts can name these
// types without an import cycle: development's needs.ts reads the copy (plans/feat-budget-burn.md F8).
export type SourceId = "requests" | "agents" | "budgets" | "sessions";

export type Severity = "action" | "watch" | "waiting";
