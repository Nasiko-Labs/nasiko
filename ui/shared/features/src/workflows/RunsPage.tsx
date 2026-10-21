// Ported from nasiko-ui-lab@1b99bdda common/src/features/workflows/RunsPage.tsx. Local edits: the lab's `EmptyState` (shared/state-card) is `LabEmptyState`; `"use client"`; scoped
// `executionsQuery`/`executionQuery`/`workflowKeys`; the page registers `useRunsTabLanding` (./run.ts: a run started
// from anywhere opens here while it is on screen, the lab's pathname check) and `setSearch` takes the `replace` option
// for it; `Button asChild` + TanStack `Link` -> `ButtonLink` (`/workflows/<id>?run=…` through `hrefFor`,
// `/sessions/<id>`); the filters' `Select` gets `items` (Base UI's trigger shows the label, not the value); the card's
// chevron reads Base UI's trigger `data-panel-open` (`group-data-[state=closed]:-rotate-90` -> `-rotate-90
// group-data-[panel-open]:rotate-0`); callbacks that returned a void expression are braced (lint); Prettier formatting.
// Legacy ui/ on development (3aac84f66, c721a11a8): the module's title and Create in the top bar, prefixed filter
// labels.
"use client";

/**
 * Every workflow run, newest first (plans/feat-workflows.md §6): the newest 50, filtered here; polls while any run is
 * still moving. A card is collapsible: a moving run opens by default, and the user's own toggle wins. On `main` the
 * list carries no `hitl` (W-4), so an opened paused run reads its own detail for the request card.
 */
import { useMutation, useMutationState, useQuery, useQueryClient } from "@tanstack/react-query";
import { ChevronDown, Info, Loader2, RotateCw, SearchX, Workflow } from "lucide-react";
import { useDeferredValue, useEffect, useState } from "react";
import { toast } from "@nasiko/ui/toast";
import { TopBar } from "@nasiko/app-core/shell/TopBar";
import { PageHeader } from "@nasiko/ui/composites";
import { PageLoader } from "@nasiko/ui/composites";
import { SearchInput } from "@nasiko/ui/composites";
import { LabEmptyState as EmptyState, StateCard } from "@nasiko/ui/composites";
import { Badge } from "@nasiko/ui/primitives";
import { Button } from "@nasiko/ui/primitives";
import { useProductScope } from "@nasiko/data-access/product";
import { hrefFor } from "@nasiko/app-core/url-state";
import { ButtonLink } from "../agents/components/ButtonLink";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@nasiko/ui/primitives";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@nasiko/ui/primitives";
import { relTime } from "../agents/format";
import { fmtDuration, fmtLongDay, fmtTokens } from "@nasiko/ui/lib";
import { useNow } from "@nasiko/app-core/useNow";
import { cn } from "@nasiko/ui/cn";
import { executionQuery, executionsQuery, runWorkflow, workflowKeys } from "./api";
import { AGE_FILTER_LABEL, STATUS_FILTER_LABEL, copy, reason } from "./copy";
import {
  execStatus,
  filterRuns,
  isExecActive,
  isOrphan,
  runStepsLabel,
  runTitle,
  showsRunError,
} from "./logic";
import { RUN_AGE_FILTERS, RUN_STATUS_FILTERS, type RunsSearch } from "./search";
import type { ExecutionRow } from "./types";
import { Chip, CreateWorkflowLink, SectionNav, ToneBadge } from "./components/bits";
import { RunSteps } from "./components/RunSteps";
import { useRunsTabLanding } from "./run";

export function RunsPage({
  search,
  setSearch,
}: {
  search: RunsSearch;
  setSearch: (patch: Partial<RunsSearch>, opts?: { replace?: boolean | undefined }) => void;
}) {
  const queryClient = useQueryClient();
  const scope = useProductScope();
  const runs = useQuery(executionsQuery(scope));
  // A run started from any page lands here while this page is on screen (`run.ts`).
  useRunsTabLanding((run) => {
    setSearch({ run }, { replace: true });
  });
  const term = search.q ?? "";
  const q = useDeferredValue(term);
  const status = search.status ?? "all";
  const age = search.age ?? "any";
  const now = useNow(60_000);
  // The user's own open/closed choice; otherwise a moving run is open.
  const [toggled, setToggled] = useState<Record<string, boolean>>({});
  const refresh = () => void queryClient.invalidateQueries({ queryKey: workflowKeys.runs(scope) });
  const rerun = useMutation({
    mutationFn: (mafId: string) => runWorkflow(mafId),
    onSuccess: () => {
      refresh();
      void queryClient.invalidateQueries({ queryKey: workflowKeys.lists(scope) });
    },
    onError: (err) => {
      toast.error(reason(err, copy.couldNotStart));
    },
  });

  // Run was just clicked (`run.ts`): a placeholder until the start answers and the list has the run.
  const starting = useMutationState({
    filters: { mutationKey: workflowKeys.start(scope), status: "pending" },
  }).length;

  const all = runs.data ?? [];
  const shown = filterRuns(all, { q, status, age }, now);
  const none = runs.isSuccess && all.length === 0 && !starting;
  const target = search.run;
  const listed = !!target && shown.some((r) => r.id === target);
  useEffect(() => {
    if (listed)
      document.querySelector(`[data-card="${target}"]`)?.scrollIntoView({ block: "nearest" });
  }, [listed, target]);

  let body;
  if (runs.isError && !runs.data)
    body = (
      <StateCard
        tone="error"
        title={copy.runsLoadFailed}
        fix={reason(runs.error)}
        action={
          <Button size="sm" variant="outline" onClick={() => void runs.refetch()}>
            <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
          </Button>
        }
      />
    );
  else if (runs.isPending) body = <PageLoader label={copy.loadingRuns} />;
  else if (none)
    body = (
      <EmptyState
        icon={Workflow}
        title={copy.noRuns}
        action={
          <ButtonLink variant="outline" href="/workflows">
            {copy.viewWorkflows}
          </ButtonLink>
        }
      >
        {copy.noRunsText}
      </EmptyState>
    );
  else if (!shown.length)
    body = (
      <EmptyState
        icon={SearchX}
        title={copy.noRunsMatch}
        action={
          <Button
            size="sm"
            variant="outline"
            onClick={() => {
              setSearch({ q: undefined, status: undefined, age: undefined });
            }}
          >
            {copy.clearFilters}
          </Button>
        }
      >
        {copy.noRunsMatchText}
      </EmptyState>
    );
  else
    body = (
      <ul data-tour="runs-list" aria-label={copy.runsList} className="flex flex-col gap-3">
        {shown.map((r) => (
          <li key={r.id}>
            <RunCard
              run={r}
              open={
                toggled[r.id] ??
                (r.id === target || r.status === "pending" || r.status === "running")
              }
              onOpenChange={(o) => {
                setToggled((t) => ({ ...t, [r.id]: o }));
              }}
              onRerun={() => {
                if (r.maf_id) rerun.mutate(r.maf_id);
              }}
              rerunning={rerun.isPending && rerun.variables === r.maf_id}
              onHitlChange={refresh}
            />
          </li>
        ))}
      </ul>
    );

  return (
    <div className="space-y-4">
      <TopBar actions={<CreateWorkflowLink />} />
      <PageHeader title={copy.title} description={copy.sub} />
      <SectionNav current="/workflows/runs" />
      {/* One row the width of the list: search on the left, the filters on the right. */}
      <div data-tour="runs-filters" className="flex flex-wrap items-center gap-2">
        <SearchInput
          aria-label={copy.runsSearch}
          placeholder={copy.searchPlaceholder}
          value={term}
          onChange={(e) => {
            setSearch({ q: e.target.value || undefined });
          }}
          disabled={none}
          className="w-full max-w-none sm:w-72"
        />
        <Select
          value={status}
          items={STATUS_FILTER_LABEL}
          onValueChange={(v) => {
            setSearch({ status: v === "all" ? undefined : (v as RunsSearch["status"]) });
          }}
          disabled={none}
        >
          <SelectTrigger size="sm" aria-label={copy.statusFilter} className="text-xs sm:ml-auto">
            <span className="text-muted-foreground">{copy.statusPrefix}:</span> <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {RUN_STATUS_FILTERS.map((v) => (
              <SelectItem key={v} value={v}>
                {STATUS_FILTER_LABEL[v]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select
          value={age}
          items={AGE_FILTER_LABEL}
          onValueChange={(v) => {
            setSearch({ age: v === "any" ? undefined : (v as RunsSearch["age"]) });
          }}
          disabled={none}
        >
          <SelectTrigger size="sm" aria-label={copy.ageFilter} className="text-xs">
            <span className="text-muted-foreground">{copy.agePrefix}:</span> <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {RUN_AGE_FILTERS.map((v) => (
              <SelectItem key={v} value={v}>
                {AGE_FILTER_LABEL[v]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {starting ? (
        <div
          role="status"
          className="flex items-center gap-3 rounded-xl border border-primary/40 bg-card p-4 text-sm text-muted-foreground"
        >
          <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden />
          {copy.startingRun}
        </div>
      ) : null}
      {body}
    </div>
  );
}

function RunCard({
  run,
  open,
  onOpenChange,
  onRerun,
  rerunning,
  onHitlChange,
}: {
  run: ExecutionRow;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onRerun: () => void;
  rerunning: boolean;
  onHitlChange: () => void;
}) {
  const orphan = isOrphan(run);
  const active = isExecActive(run.status);
  const status = execStatus(run.status);
  // `main`'s list has no `hitl` (W-4): an open paused run reads its detail, which does.
  const detail = useQuery({
    ...executionQuery(useProductScope(), run.id),
    enabled: open && run.status === "awaiting_human" && !run.hitl,
  });
  const hitl = run.hitl ?? detail.data?.hitl;
  const meta = [
    runStepsLabel(run),
    active ? copy.started(relTime(run.created_at)) : fmtLongDay(run.created_at),
    run.duration_ms != null ? fmtDuration(run.duration_ms) : "",
    run.tokens_used ? copy.tokens(fmtTokens(run.tokens_used)) : "",
  ].filter((m): m is string => !!m);
  return (
    <Collapsible
      open={open}
      onOpenChange={onOpenChange}
      data-card={run.id}
      className={cn(
        "rounded-xl border bg-card",
        // A tinted hairline, not a ring (user request 2026-10-02: the 2 px ring read too bold).
        run.status === "awaiting_human" && "border-warning/60",
        // The one being read wins over "needs attention".
        open && "border-primary/40",
      )}
    >
      <div className="flex flex-wrap items-start gap-x-3 gap-y-2 p-4">
        <div className="flex min-w-0 flex-1 flex-col gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="min-w-0 text-base font-medium">
              <CollapsibleTrigger className="group flex max-w-full items-center gap-2 rounded-md text-left outline-none focus-visible:ring-2 focus-visible:ring-ring">
                <ChevronDown
                  aria-hidden
                  className="size-4 shrink-0 -rotate-90 transition-transform group-data-[panel-open]:rotate-0 motion-reduce:transition-none"
                />
                <span className="truncate">{runTitle(run)}</span>
              </CollapsibleTrigger>
            </h2>
            {orphan ? (
              <Badge variant="destructive">
                <Info aria-hidden /> {copy.orphan}
              </Badge>
            ) : null}
          </div>
          <div className="flex flex-wrap items-center gap-1.5 pl-6">
            {meta.map((m) => (
              <Chip key={m}>{m}</Chip>
            ))}
            <ToneBadge tone={status.tone}>{status.label}</ToneBadge>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {!orphan && run.maf_id ? (
            <ButtonLink
              variant="outline"
              size="sm"
              href={hrefFor(`/workflows/${encodeURIComponent(run.maf_id)}`, { run: run.id })}
            >
              {run.workflow_status === "draft" ? copy.openDraft : copy.open}
            </ButtonLink>
          ) : null}
          {!orphan && run.maf_id && (run.status === "failed" || run.status === "stopped") ? (
            <Button variant="outline" size="sm" disabled={rerunning} onClick={onRerun}>
              <RotateCw className={cn(rerunning && "animate-spin")} aria-hidden /> {copy.rerun}
            </Button>
          ) : null}
          {/* The execution id is the run's session id. */}
          <ButtonLink variant="ghost" size="sm" href={`/sessions/${encodeURIComponent(run.id)}`}>
            {copy.viewTrace}
          </ButtonLink>
        </div>
      </div>
      <CollapsibleContent className="flex flex-col gap-3 border-t px-4 pt-4 pb-2">
        <RunSteps
          steps={run.step_results ?? []}
          totalTokens={run.tokens_used ?? 0}
          hitl={hitl}
          onHitlChange={onHitlChange}
        />
        {showsRunError(run) ? (
          <p className="mb-2 rounded-md border border-destructive/30 bg-destructive/5 p-3 text-sm break-words whitespace-pre-wrap text-destructive">
            {run.error}
          </p>
        ) : null}
      </CollapsibleContent>
    </Collapsible>
  );
}
