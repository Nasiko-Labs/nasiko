// Ported from nasiko-ui-lab@1b99bdda common/src/features/workflows/api.ts. Local edits: every key is scoped
// (`productKey(scope, 'workflows', …the lab's parts)`, `workflowKeys.*` take the scope, `all` is the feature prefix),
// so the query builders (`workflowListQuery`, `workflowQuery`, `executionQuery`, `executionsQuery`, which becomes a
// function) and `dropFromLists` take it first; `json()` leaves `body` out when there is none, and the write bodies' optional fields admit `undefined` (exactOptionalPropertyTypes); Prettier formatting.
/**
 * Workflow queries and mutations (plans/feat-workflows.md). Every read is under `['workflows', …]`. Lists are keyed by
 * sort, so a late answer for an old sort never overwrites the new one. Runs poll while they move.
 */
import { keepPreviousData, queryOptions, type QueryClient } from "@tanstack/react-query";
import type { ResolveBody } from "../chat/api";
import type { HitlDto } from "../chat/types";
import type { Scope } from "@nasiko/data-access";
import {
  apiData,
  apiFetch,
  productKey,
  productPrefix,
  withQuery,
} from "@nasiko/data-access/product";
import { isExecActive } from "./logic";
import type { DraftSort, WorkflowSort } from "./search";
import { LIST_LIMIT, RUNS_LIMIT, RUN_POLL_MS } from "./tuning";
import {
  executionListSchema,
  executionSchema,
  planSchema,
  runStartedSchema,
  workflowListSchema,
  workflowSchema,
  type Execution,
  type ExecutionRow,
  type GeneratedPlan,
  type RunStarted,
  type StepInput,
  type Workflow,
  type WorkflowRow,
} from "./types";

const enc = encodeURIComponent;
const W = (id: string) => `/api/maf/workflow/${enc(id)}`;

export const WORKFLOWS_FEATURE = "workflows";
const k = (scope: Scope, ...parts: unknown[]) => productKey(scope, WORKFLOWS_FEATURE, ...parts);

export const workflowKeys = {
  all: (scope: Scope) => productPrefix(scope, WORKFLOWS_FEATURE),
  lists: (scope: Scope) => k(scope, "list"),
  list: (scope: Scope, mode: "deployed" | "drafts", sort: string) => k(scope, "list", mode, sort),
  detail: (scope: Scope, id: string) => k(scope, "detail", id),
  runs: (scope: Scope) => k(scope, "runs"),
  runList: (scope: Scope) => k(scope, "runs", "list"),
  run: (scope: Scope, id: string) => k(scope, "runs", id),
  /** The Run button's mutation (`run.ts`), read by the run page while it starts. */
  start: (scope: Scope) => k(scope, "start"),
};

const json = (method: string, body?: unknown): RequestInit => ({
  method,
  headers: { "Content-Type": "application/json" },
  ...(body === undefined ? {} : { body: JSON.stringify(body) }),
});

const rows = async (path: string, signal: AbortSignal) =>
  (await apiData<{ data: WorkflowRow[] }>(path, { signal, schema: workflowListSchema })).data;

/** Deployed (`GET /maf/workflows`, active rows only) or drafts (NAS-697; W-1). The first 100 either way. */
export const workflowListQuery = (
  scope: Scope,
  p: { mode: "deployed"; sort: WorkflowSort } | { mode: "drafts"; sort: DraftSort },
) =>
  queryOptions({
    queryKey: workflowKeys.list(scope, p.mode, p.sort),
    queryFn: ({ signal }) =>
      rows(
        withQuery(p.mode === "deployed" ? "/api/maf/workflows" : "/api/maf/workflow/drafts", {
          limit: LIST_LIMIT,
          offset: 0,
          sort: p.sort,
        }),
        signal,
      ),
    placeholderData: keepPreviousData,
  });

export const workflowQuery = (scope: Scope, id: string) =>
  queryOptions({
    queryKey: workflowKeys.detail(scope, id),
    queryFn: ({ signal }) => apiData<Workflow>(W(id), { signal, schema: workflowSchema }),
  });

/** One run; polls while it is still moving (paused included). */
export const executionQuery = (scope: Scope, id: string) =>
  queryOptions({
    queryKey: workflowKeys.run(scope, id),
    queryFn: ({ signal }) =>
      apiData<Execution>(`/api/maf/execution/${enc(id)}`, { signal, schema: executionSchema }),
    refetchInterval: (q) => (isExecActive(q.state.data?.status) ? RUN_POLL_MS : false),
  });

/** The newest 50 runs of every workflow, deleted ones included; polls while any of them moves. */
export const executionsQuery = (scope: Scope) =>
  queryOptions({
    queryKey: workflowKeys.runList(scope),
    queryFn: async ({ signal }) =>
      (
        await apiData<{ data: ExecutionRow[] }>(
          withQuery("/api/maf/executions", { limit: RUNS_LIMIT, offset: 0 }),
          { signal, schema: executionListSchema },
        )
      ).data,
    refetchInterval: (q) =>
      q.state.data?.some((r) => isExecActive(r.status)) ? RUN_POLL_MS : false,
  });

export const createWorkflow = (body: {
  name: string;
  description?: string | undefined;
  steps: StepInput[];
}) => apiData<Workflow>("/api/maf/workflows", { ...json("POST", body), schema: workflowSchema });

/** Absent = keep; `description: null` clears; `steps: []` is a 400. */
export const updateWorkflow = (
  id: string,
  body: {
    name?: string | undefined;
    description?: string | null | undefined;
    steps?: StepInput[] | undefined;
  },
) => apiData<Workflow>(W(id), { ...json("PUT", body), schema: workflowSchema });

/** NAS-697: a bare-sentence draft, named after the instruction's first 60 characters (a 404 = the draft is gone). */
export const saveDraft = (body: { instruction: string; draft_id?: string }) =>
  apiData<Workflow>("/api/maf/workflow/draft", { ...json("POST", body), schema: workflowSchema });

/** NAS-697: a status flip only (same id, no re-routing). */
export const promoteWorkflow = (id: string) =>
  apiData<Workflow>(`${W(id)}/promote`, { ...json("POST"), schema: workflowSchema });

/** A soft delete; answers `data: null`, so not `apiData`. */
export const deleteWorkflow = (id: string) => apiFetch<unknown>(W(id), { method: "DELETE" });

/** 202: queues a run of the stored row. */
export const runWorkflow = (id: string) =>
  apiData<RunStarted>(`${W(id)}/run`, { ...json("POST"), schema: runStartedSchema });

/** The planner's proposal (never stored); callers branch on 503 / 400 / 422 (`planFailure`). */
export const generateWorkflow = (description: string) =>
  apiData<GeneratedPlan>("/api/maf/generate", {
    ...json("POST", { description }),
    schema: planSchema,
    // An LLM call: longer than the default deadline.
    timeout: 120_000,
  });

/** A paused step's request (`router/hitl.rs`): answered in place, and the resume continues the run server-side. */
export const resolveHitl = (id: string, body: ResolveBody) =>
  apiFetch<HitlDto>(`/api/hitl/${enc(id)}/resolve`, json("POST", body));
export const cancelHitl = (id: string) =>
  apiFetch<HitlDto>(`/api/hitl/${enc(id)}/cancel`, json("POST"));

/** Gone from every cached list, without a refetch. */
export function dropFromLists(client: QueryClient, scope: Scope, id: string) {
  client.setQueriesData<WorkflowRow[]>({ queryKey: workflowKeys.lists(scope) }, (list) =>
    list?.filter((r) => r.id !== id),
  );
}
