// Ported from nasiko-ui-lab@1b99bdda common/src/features/workflows/WorkflowPage.tsx. Local edits: the lab's `EmptyState` (shared/state-card) is `LabEmptyState`; `"use client"`;
// TanStack's `useNavigate`/`useRouter` -> `useRouter()` from the navigation port (`@nasiko/app-core/navigation`) (`push`/`replace` with `scroll: false`,
// `router.history.back()` -> `back()`); `Button asChild` + `Link` -> `ButtonLink`; the run's Back reads
// `workflowRunPushed` from `window.history.state` (the lab's TanStack `HistoryState` augmentation; nothing in the lab
// sets it, so Back replaces with the review there too); scoped `workflowQuery`/`dropFromLists`; the lab's `ApiError` is
// `ProductApiError`; optional props admit `undefined`; Prettier formatting.
"use client";

/**
 * One workflow (plans/feat-workflows.md §5): its review (read-only when live, the editor while a draft) or, with
 * `?run=`, one of its runs. Opening a run pushes history; the run's Back pops what this page pushed and replaces
 * otherwise (a deep link, the runs page). A 404 or 403 is one dead end (the server never says which to a stranger).
 */
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@nasiko/app-core/navigation";
import { ChevronLeft, Pencil, Play, RotateCw, Workflow as WorkflowIcon } from "lucide-react";
import { useState } from "react";
import { PageLoader } from "@nasiko/ui/composites";
import { LabEmptyState as EmptyState, StateCard } from "@nasiko/ui/composites";
import { Button } from "@nasiko/ui/primitives";
import { hrefFor } from "@nasiko/app-core/url-state";
import { ButtonLink } from "../agents/components/ButtonLink";
import { isUuid } from "../agents/normalize";
import { ProductApiError, useProductScope } from "@nasiko/data-access/product";
import { dropFromLists, workflowQuery } from "./api";
import { copy, reason } from "./copy";
import { descriptionOf, isDeployed } from "./logic";
import { useStartRun } from "./run";
import type { Workflow } from "./types";
import { DeleteWorkflowDialog } from "./components/DeleteWorkflowDialog";
import { EditFace } from "./components/EditFace";
import { RunView } from "./components/RunView";

/** The lab's `location.state.workflowRunPushed`: this entry is a run the page pushed itself (nothing sets it in the lab
 *  either, so Back from a run replaces with the review). */
const runPushed = () =>
  (window.history.state as { workflowRunPushed?: unknown } | null)?.workflowRunPushed === true;

const COLUMN = "mx-auto flex w-full max-w-3xl flex-col gap-6 [&>*]:min-w-0";

export function WorkflowPage({ id, run }: { id: string; run?: string | undefined }) {
  // The server's path parameter is a UUID (anything else is a plain-text 400).
  if (!isUuid(id)) return <DeadEnd title={copy.notFound} text={copy.notFoundText} />;
  return <Detail id={id.toLowerCase()} run={run} />;
}

function DeadEnd({ title, text }: { title: string; text: string }) {
  return (
    <div className={COLUMN}>
      <EmptyState
        icon={WorkflowIcon}
        title={<h1>{title}</h1>}
        action={
          <ButtonLink variant="outline" href="/workflows">
            {copy.backToWorkflows}
          </ButtonLink>
        }
      >
        {text}
      </EmptyState>
    </div>
  );
}

function Detail({ id, run }: { id: string; run?: string | undefined }) {
  const wf = useQuery(workflowQuery(useProductScope(), id));
  if (wf.data) return <Loaded key={wf.data.id} wf={wf.data} run={run} />;
  if (wf.isError)
    return wf.error instanceof ProductApiError &&
      (wf.error.status === 404 || wf.error.isForbidden) ? (
      <DeadEnd title={copy.notFound} text={copy.notFoundText} />
    ) : (
      <div className={COLUMN}>
        <h1 className="sr-only">{copy.loadOneFailed}</h1>
        <StateCard
          tone="error"
          title={copy.loadOneFailed}
          fix={reason(wf.error)}
          action={
            <Button size="sm" variant="outline" onClick={() => void wf.refetch()}>
              <RotateCw className="size-3.5" aria-hidden /> {copy.retry}
            </Button>
          }
        />
      </div>
    );
  return <PageLoader label={copy.loadingWorkflow} />;
}

function Loaded({ wf, run }: { wf: Workflow; run?: string | undefined }) {
  const router = useRouter();
  const [editing, setEditing] = useState(false);
  const live = isDeployed(wf);
  // Save & run lands on the Runs tab with the run open, as Run does (`run.ts`).
  const openRun = (execId: string) => {
    router.push(hrefFor("/workflows/runs", { run: execId }), { scroll: false });
  };
  if (run)
    return (
      <RunView
        wf={wf}
        exec={run}
        onBack={() => {
          if (runPushed()) router.back();
          else router.replace(`/workflows/${encodeURIComponent(wf.id)}`, { scroll: false });
        }}
      />
    );
  return (
    <div className={COLUMN}>
      <title>{`${wf.name} · OpenRuntime`}</title>
      <div>
        <ButtonLink
          variant="ghost"
          size="sm"
          className="-ml-2 text-muted-foreground"
          href={live ? "/workflows" : "/workflows/drafts"}
        >
          <ChevronLeft aria-hidden /> {live ? copy.crumbDeployed : copy.crumbDrafts}
        </ButtonLink>
      </div>
      {live && !editing ? (
        <ReadOnlyFace
          wf={wf}
          onEdit={() => {
            setEditing(true);
          }}
        />
      ) : (
        <EditFace
          wf={wf}
          onCancel={() => {
            setEditing(false);
          }}
          onRun={openRun}
        >
          <OutputGuidelines wf={wf} />
        </EditFace>
      )}
      <DangerZone wf={wf} />
    </div>
  );
}

function OutputGuidelines({ wf }: { wf: Workflow }) {
  const text = wf.maf_json?.output_generation;
  if (!text) return null;
  return (
    <section aria-labelledby="output-guidelines" className="flex flex-col gap-2">
      <h2 id="output-guidelines" className="text-base font-medium">
        {copy.outputGuidelines}
      </h2>
      <p className="text-sm whitespace-pre-wrap text-muted-foreground">{text}</p>
    </section>
  );
}

/** A live workflow as it stands: Edit to change it, Run to run it. */
function ReadOnlyFace({ wf, onEdit }: { wf: Workflow; onEdit: () => void }) {
  const steps = wf.maf_json?.steps ?? [];
  const startRun = useStartRun();
  return (
    <>
      <div className="flex flex-col gap-2">
        <h1 tabIndex={-1} className="text-xl font-semibold break-words outline-none">
          {wf.name}
        </h1>
        {descriptionOf(wf) ? (
          <p className="text-sm text-muted-foreground">{descriptionOf(wf)}</p>
        ) : null}
      </div>
      <section aria-labelledby="steps-title" className="flex flex-col gap-3">
        <h2 id="steps-title" className="text-base font-medium">
          {copy.stepsTitle}
        </h2>
        {steps.length ? (
          <ol className="flex flex-col gap-3">
            {steps.map((s, i) => (
              <li key={s.step_id} className="flex flex-col gap-1 rounded-xl border p-4">
                <h3 className="text-sm font-medium">{copy.step(i + 1)}</h3>
                <p className="text-sm break-words">{s.task_description}</p>
                <p className="text-xs text-muted-foreground">
                  {s.agent_name || copy.assignedWhenSaved}
                </p>
              </li>
            ))}
          </ol>
        ) : (
          <EmptyState title={copy.noSteps}>{copy.noStepsLive}</EmptyState>
        )}
      </section>
      <OutputGuidelines wf={wf} />
      <div className="flex flex-wrap justify-end gap-2">
        <Button variant="ghost" onClick={onEdit}>
          <Pencil aria-hidden /> {copy.edit}
        </Button>
        <Button
          disabled={!steps.length}
          onClick={() => {
            startRun(wf.id);
          }}
        >
          <Play aria-hidden /> {copy.run}
        </Button>
      </div>
    </>
  );
}

function DangerZone({ wf }: { wf: Workflow }) {
  const router = useRouter();
  const queryClient = useQueryClient();
  const scope = useProductScope();
  const [asking, setAsking] = useState<Workflow | null>(null);
  const live = isDeployed(wf);
  return (
    <section
      aria-labelledby="danger-zone"
      className="flex flex-col gap-3 rounded-xl border border-destructive/30 p-4"
    >
      <h2 id="danger-zone" className="text-base font-medium">
        {copy.dangerZone}
      </h2>
      <p className="text-sm text-muted-foreground">
        {live ? copy.dangerLive : copy.dangerDraft} {copy.dangerAfter}
      </p>
      <div>
        <Button
          variant="outline"
          className="border-destructive/40 text-destructive hover:text-destructive"
          onClick={() => {
            setAsking(wf);
          }}
        >
          {copy.delete}
        </Button>
      </div>
      <DeleteWorkflowDialog
        workflow={asking}
        onClose={() => {
          setAsking(null);
        }}
        onDeleted={() => {
          dropFromLists(queryClient, scope, wf.id);
          router.push(live ? "/workflows" : "/workflows/drafts", { scroll: false });
        }}
      />
    </section>
  );
}
