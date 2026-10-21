// Ported from the legacy ui/ on development (3aac84f66) common/src/features/tour/tours.test.tsx (dd9fe0f64). Local edits:
// React mocks and harness imports; no TokenOps nav group; the agent facts are on the Overview tab.
/**
 * Each tour points at elements by `data-tour` (tours.ts). A target that disappears from its page only drops a step,
 * silently, so this renders every page on the seed and checks its targets are still there.
 */
import { waitFor } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { buildId } from "@nasiko/mocks/deploy";
import { flowsMockState, mcpMockState, workflowsMockState } from "@nasiko/mocks/handlers";
import { SHOWCASE_SESSION } from "@nasiko/mocks/observability";
import { seed, setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { renderApp } from "@nasiko/app-shell/testing/render-app";
import { TOURS } from "./tours";

setupPinnedSeed();

const PAGES: Record<string, readonly string[]> = {
  "/": [
    "app-menu",
    "nav-work",
    "nav-fleet",
    "nav-observe",
    "nav-connect",
    "nav-org",
    "tour-button",
    "overview-range",
    "overview-summary",
    "overview-needs",
    "overview-health",
    "overview-actions",
  ],
  "/chat": ["chat-rail", "chat-target", "chat-composer"],
  "/agents": ["section-nav", "page-actions", "agents-toolbar", "agents-grid"],
  "/agents/mine": ["mine-status-tabs", "mine-table"],
  [`/agents/${seed.agents[0]?.id}`]: ["agent-header", "agent-tabs", "agent-facts"],
  "/deploy": ["deploy-methods", "deploy-upload-form", "deploy-requirements", "deploy-cli"],
  "/builds": ["builds-filters", "builds-panel"],
  "/workflows": ["workflows-toolbar", "workflows-list"],
  "/workflows/runs": ["runs-filters", "runs-list"],
  "/router": ["router-how", "router-tabs", "router-agent-filters", "router-agents"],
  "/mcp": ["mcp-toolbar", "mcp-grid"],
  "/sessions": ["sessions-toolbar", "sessions-list"],
  [`/sessions/${SHOWCASE_SESSION}`]: ["session-narrative", "session-spans", "session-span-panel"],
  "/flows": ["flows-charts", "flows-table"],
  "/tokenops": ["tokenops-filters", "tokenops-summary", "tokenops-sections"],
  "/optimization": ["optimization-header", "optimization-lead", "optimization-settings"],
  "/harnesses": ["harnesses-header", "harnesses-summary", "harnesses-cards"],
  "/settings": ["settings-sections", "settings-form", "settings-save"],
  "/settings/secrets": ["secrets-list", "secrets-add"],
  "/account/appearance": ["settings-sections", "appearance-mode", "appearance-theme"],
  "/account/password": ["password-form"],
  "/status": ["status-backend"],
  "/workflows/new": ["wf-name", "wf-plan", "wf-steps", "wf-save"],
};
/** Detail pages: their ids come from the mocks, built per test. */
const DETAILS: [string, () => string, readonly string[]][] = [
  // Build 3 has failed: a finished build shows its outcome.
  ["build", () => `/builds/${buildId(3)}`, ["page-actions", "build-outcome", "build-stages"]],
  [
    "deployed workflow",
    () => `/workflows/${workflowsMockState().workflows.find((w) => w.status === "active")?.id}`,
    ["wf-steps", "wf-run", "wf-danger"],
  ],
  [
    "draft workflow",
    () => `/workflows/${workflowsMockState().workflows.find((w) => w.status === "draft")?.id}`,
    ["wf-steps", "wf-plan", "wf-danger"],
  ],
  [
    "MCP server",
    () => `/mcp/${mcpMockState().connectors.find((c) => c.name === "docs-search")?.id}`,
    ["mcp-detail-header", "mcp-detail-actions", "mcp-detail-tabs", "mcp-detail-overview"],
  ],
  [
    "flow",
    () => `/flows/${flowsMockState().flows.find((f) => f.title?.startsWith("Refund order"))?.id}`,
    ["flow-narrative", "flow-kpis", "flow-view"],
  ],
];
/** Only in states the seed doesn't open on: a list's Previous/Next, the empty fleet's setup card. */
const CONDITIONAL = ["page-stepper", "overview-setup-guide"];

const has = (target: string) => document.querySelector(`[data-tour~="${target}"]`) !== null;

describe("tour targets", () => {
  it("every target is checked on some page", () => {
    const checked = new Set([
      ...Object.values(PAGES).flat(),
      ...DETAILS.flatMap(([, , targets]) => targets),
      ...CONDITIONAL,
    ]);
    const targets = Object.values(TOURS)
      .flat()
      .flatMap((s) => (s.target ? [s.target] : []));
    expect(targets.filter((t) => !checked.has(t))).toEqual([]);
  });

  it.each(Object.entries(PAGES))("%s has its targets", async (url, targets) => {
    renderApp(url);
    await waitFor(() => expect(targets.filter((t) => !has(t))).toEqual([]));
  });

  it.each(DETAILS)("a %s has its targets", async (_, url, targets) => {
    renderApp(url());
    await waitFor(() => expect(targets.filter((t) => !has(t))).toEqual([]));
  });
});
