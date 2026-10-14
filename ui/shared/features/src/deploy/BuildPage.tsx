// Ported from nasiko-ui-lab@1b99bdda common/src/features/deploy/BuildPage.tsx. Local edits: links are the navigation port's `Link` (`@nasiko/app-core/navigation`)
// (`/builds`, `/agents/<id>`) and `Button asChild` + `Link` -> `ButtonLink` (../agents/components/ButtonLink.tsx),
// the Deploy-again and Chat targets built with `hrefFor`; `BreadcrumbLink asChild` -> `render={<Link …/>}` (Base UI
// useRender); the lab's `AGENTS_DIRECTORY_KEY` is the scoped `agentKeys.directory(scope)` (read where the page and
// `useAgentSettle` invalidate it); Prettier formatting.
"use client";

/**
 * One build (plans/feat-deploy.md §5): the title, one row of four progress segments (design review 14), the outcome
 * directly under it (3), and full-width Details with the build-logs line (10, 11).
 *
 * Live status: the deploy stream while the build runs; a dropped stream falls back to polling the upload row; after
 * the image is built, the agent's own status decides Running vs "Built, but not running". Stage times exist only for
 * transitions this page saw (the server keeps none, D-4); a build opened after it finished shows its total time.
 */
import { useQueryClient } from "@tanstack/react-query";
import Link from "@nasiko/app-core/navigation";
import { AlertTriangle, CheckCircle2, Info, RotateCw } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { PageHeader } from "@nasiko/ui/composites";
import { PanelError } from "@nasiko/ui/composites";
import { PageLoader } from "@nasiko/ui/composites";
import { StateCard } from "@nasiko/ui/composites";
import { Alert, AlertDescription, AlertTitle } from "@nasiko/ui/primitives";
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbSeparator,
} from "@nasiko/ui/primitives";
import { Card } from "@nasiko/ui/primitives";
import { relTime } from "../agents/format";
import { useNow } from "@nasiko/app-core/useNow";
import { agentKeys, useAgentsDirectory } from "../agents/api";
import { useProductScope } from "@nasiko/data-access/product";
import { hrefFor } from "@nasiko/app-core/url-state";
import { ButtonLink } from "../agents/components/ButtonLink";
import { useBuild, useBuildStream, useUploadStatus } from "./api";
import { copy } from "./copy";
import { explainError } from "./errors";
import { clonedRepo } from "./uploads";
import { setOpenBuild } from "./follower";
import { buildSource, deriveBuild, fmtElapsed, isActive, repoOf, type StageId } from "./steps";
import {
  AGENT_SETTLE_MAX_MS,
  AGENT_SETTLE_POLL_MS,
  BUILD_TIMEOUT_MIN,
  ELAPSED_TICK_MS,
  SLOW_BUILD_MS,
} from "./tuning";
import { BuildBadge } from "./components/BuildBadge";
import { ProgressSegments } from "./components/ProgressSegments";

export function BuildPage({ buildId }: { buildId: string }) {
  const qc = useQueryClient();
  const scope = useProductScope();
  /** The agent directory's query key (agents/api.ts `agentKeys.directory`), invalidated exactly like the Overview does. */
  const DIRECTORY = agentKeys.directory(scope);
  const [dropped, setDropped] = useState(false);
  // A dropped stream isn't reopened: the record and the upload row are polled instead until the build finishes.
  const build = useBuild(buildId, dropped);
  const record = build.data && build.data.available ? build.data.value : null;
  // The stream runs while the record says the build is in progress (it closes itself on the terminal frame).
  const stream = useBuildStream(buildId, !!record && isActive(record.status));
  if (stream.kind === "dropped" && !dropped) setDropped(true);
  // No build row (a failed first upload deleted it, D-4) or a dropped stream: poll the upload row until it finishes.
  const upload = useUploadStatus(buildId, dropped || build.data === null);

  // A dropped stream's last status is only a fallback: the polled record wins once it's terminal (newest wins).
  const status =
    stream.kind === "live" || stream.kind === "done"
      ? stream.status
      : stream.kind === "dropped"
        ? record && !isActive(record.status)
          ? record.status
          : stream.status
        : null;
  const dir = useAgentsDirectory();
  const agent = record ? dir.byId.get(record.agent_id) : undefined;
  // The record's github_url (builds made with POST /api/builds), else a clone started in this tab (D-12).
  const repo = record?.github_url ? repoOf(record.github_url) : clonedRepo(buildId);
  const built = (status ?? record?.status) === "success";
  const view = deriveBuild({
    status: status ?? record?.status ?? null,
    upload: upload.data ?? null,
    agentStatus: built ? (agent?.status ?? null) : null,
  });

  // This page streams its own build: the background follower drops it without a toast (eng review R8).
  useEffect(() => {
    setOpenBuild(buildId);
    return () => {
      setOpenBuild(null);
    };
  }, [buildId]);

  // After the image is built, re-read the agent until it's running or down (per-observer interval on the shared directory).
  // The cached directory can predate this deploy (a redeploy of a crashed agent): re-read it once the image is built, and
  // keep settling while the agent reads down but the upload says the deploy completed.
  useEffect(() => {
    if (built) void qc.invalidateQueries({ queryKey: DIRECTORY, exact: true });
  }, [built, qc, DIRECTORY]);
  useAgentSettle(
    built &&
      (!view.terminal || (view.outcome === "notRunning" && upload.data?.status === "completed")),
  );
  useEffect(() => {
    // The build finished here: the catalog and the Overview should see the new version at once.
    if (view.outcome === "running") void qc.invalidateQueries({ queryKey: DIRECTORY, exact: true });
  }, [view.outcome, qc, DIRECTORY]);

  // Stage times this page saw live (the server has none).
  const seen = useRef(new Map<StageId, number>());
  const clock = useNow(view.terminal ? null : ELAPSED_TICK_MS);
  const currentStage = view.current;
  useEffect(() => {
    // eslint-disable-next-line react-hooks/purity -- runs in an effect (the stage's first sighting), not during render
    if (currentStage && !seen.current.has(currentStage)) seen.current.set(currentStage, Date.now());
  }, [currentStage]);

  if (build.isPending) return <PageLoader label={copy.build.loading} />;
  if (build.isError)
    return (
      <Frame>
        <PanelError
          error={build.error}
          onRetry={() => void build.refetch()}
          what={copy.build.what}
        />
      </Frame>
    );
  if (build.data && !build.data.available)
    return (
      <Frame>
        <StateCard title={copy.builds.noRights} />
      </Frame>
    );
  if (!record && !upload.data && !upload.isPending) {
    return (
      <Frame>
        <StateCard
          title={copy.build.notFound}
          action={
            <ButtonLink
              href="/builds"
              size="sm"
              variant="outline"
              className="pointer-coarse:min-h-11"
            >
              {copy.build.allBuilds}
            </ButtonLink>
          }
        >
          {copy.build.notFoundHint}
        </StateCard>
      </Frame>
    );
  }

  const name = agent
    ? agent.display_name || agent.name
    : (upload.data?.agent_name ?? copy.builds.unknownAgent);
  const version = record?.version_tag ?? "";
  const created = Date.parse(record?.created_at ?? upload.data?.created_at ?? "") || clock;
  const updated = Date.parse(record?.updated_at ?? upload.data?.updated_at ?? "") || clock;
  const elapsed = Math.max(0, clock - created);
  const slow = !view.terminal && elapsed > SLOW_BUILD_MS;
  // eslint-disable-next-line @typescript-eslint/no-unnecessary-condition -- defensive read of server data (the lab's)
  const error = view.outcome === "failed" ? explainError(upload.data?.error_details?.[0]) : null;
  const title = `${name} ${version}`.trim();

  const sub = (s: (typeof view.stages)[number]) => {
    if (s.state === "current")
      return `${fmtElapsed(elapsed)} · ${slow ? copy.build.slow(`${BUILD_TIMEOUT_MIN} min`) : copy.build.usual}`;
    if (s.state === "done") {
      const at = seen.current.get(s.id);
      const nextId = view.stages[view.stages.indexOf(s) + 1]?.id;
      const end = nextId ? seen.current.get(nextId) : undefined;
      return at && end ? `${copy.build.done} · ${fmtElapsed(end - at)}` : copy.build.done;
    }
    if (s.state === "pending") return copy.build.pending;
    return null;
  };

  return (
    <Frame>
      <PageHeader
        breadcrumb={<Crumbs title={title} />}
        title={
          <span className="flex flex-wrap items-center gap-2">
            {title} <BuildBadge badge={view.badge} />
          </span>
        }
        description={[
          repo
            ? copy.source.github(repo, record?.commit_hash ?? null)
            : upload.data
              ? copy.build.uploaded(upload.data.source_info.filename)
              : record
                ? buildSource(record)
                : null,
          view.terminal
            ? copy.build.finishedIn(fmtElapsed(updated - created))
            : copy.build.started(
                relTime(new Date(created).toISOString(), Math.max(clock, created)),
              ),
        ]
          .filter(Boolean)
          .join(" · ")}
      />
      <ProgressSegments stages={view.stages} sub={sub} />
      {/* The one polite region: a success isn't an Alert, and the follower doesn't toast the open build. */}
      <p className="sr-only" role="status">
        {view.outcome === "running" ? copy.build.running(name) : ""}
      </p>

      {view.outcome === "running" ? (
        <Card
          className="flex-row flex-wrap items-center justify-between gap-3 p-4 motion-safe:animate-in motion-safe:duration-200 motion-safe:fade-in"
          data-testid="build-outcome"
          data-outcome="running"
        >
          <p className="flex items-center gap-2 font-medium">
            <CheckCircle2 className="size-5 text-success" aria-hidden />
            {copy.build.running(name)}
          </p>
          <div className="flex flex-wrap gap-2 max-sm:w-full max-sm:flex-col">
            {agent ? (
              <ButtonLink
                href={hrefFor("/chat", { agent: agent.id })}
                className="pointer-coarse:min-h-11"
              >
                {copy.build.chat}
              </ButtonLink>
            ) : null}
            {agent ? (
              <ButtonLink
                href={`/agents/${encodeURIComponent(agent.id)}`}
                variant="outline"
                className="pointer-coarse:min-h-11"
              >
                {copy.build.openAgent}
              </ButtonLink>
            ) : null}
          </div>
        </Card>
      ) : view.outcome === "notRunning" ? (
        <Alert
          className="gap-y-2 border-warning/40 bg-warning/5 [&>svg]:text-warning"
          data-testid="build-outcome"
          data-outcome="notRunning"
        >
          <AlertTriangle aria-hidden />
          <AlertTitle className="line-clamp-none">{copy.build.notRunning}</AlertTitle>
          <AlertDescription className="gap-2">
            <p>{copy.build.notRunningHint}</p>
            {agent ? (
              <ButtonLink
                href={`/agents/${encodeURIComponent(agent.id)}`}
                size="sm"
                variant="outline"
                className="pointer-coarse:min-h-11"
              >
                {copy.build.openAgent}
              </ButtonLink>
            ) : null}
          </AlertDescription>
        </Alert>
      ) : view.outcome === "failed" && error ? (
        <Alert
          className="gap-y-2 border-destructive/30 bg-destructive/5 [&>svg]:text-destructive"
          data-testid="build-outcome"
          data-outcome="failed"
        >
          <AlertTriangle aria-hidden />
          <AlertTitle className="line-clamp-none">{error.problem}</AlertTitle>
          <AlertDescription className="gap-2">
            <p>{error.fix}</p>
            {/* The one action (design review 3): Deploy as vX for a version clash, else Deploy again, with the name kept. */}
            <ButtonLink
              href={hrefFor("/deploy", {
                ...(repo ? { method: "github" as const, repo } : {}),
                name: agent?.name ?? upload.data?.agent_name,
                version: error.suggested,
              })}
              size="sm"
              className="pointer-coarse:min-h-11"
            >
              <RotateCw className="size-3.5" aria-hidden />{" "}
              {error.suggested ? copy.build.deployAs(error.suggested) : copy.build.deployAgain}
            </ButtonLink>
          </AlertDescription>
        </Alert>
      ) : (
        <p className="flex items-center gap-2 text-sm text-muted-foreground">
          <Info className="size-4" aria-hidden />
          {copy.build.leave}
        </p>
      )}

      <Card className="gap-3 p-4">
        <h2 className="text-sm font-semibold">{copy.build.details}</h2>
        <dl className="grid gap-x-8 gap-y-2 text-sm sm:grid-cols-[auto_1fr] md:flex md:flex-wrap md:gap-x-10">
          <Detail label={copy.build.source}>
            {repo
              ? copy.source.github(repo, record?.commit_hash ?? null)
              : record
                ? buildSource(record)
                : copy.source.upload}
          </Detail>
          <Detail label={copy.build.version}>{version || "—"}</Detail>
          <Detail label={copy.build.agent}>
            {agent ? (
              <Link
                href={`/agents/${encodeURIComponent(agent.id)}`}
                className="text-primary-text underline-offset-4 hover:underline"
              >
                {agent.display_name || agent.name}
              </Link>
            ) : (
              name
            )}
          </Detail>
        </dl>
        <p className="border-t pt-3 text-xs text-muted-foreground">
          {copy.build.logs} · {copy.build.logsUnavailable}
        </p>
      </Card>
    </Frame>
  );
}

function useAgentSettle(active: boolean) {
  const qc = useQueryClient();
  const scope = useProductScope();
  useEffect(() => {
    if (!active) return;
    // The agent may never settle (deleted, not in this viewer's directory, stuck deploying): stop after a while, and
    // skip ticks while the tab is hidden.
    const until = Date.now() + AGENT_SETTLE_MAX_MS;
    const t = setInterval(() => {
      if (Date.now() > until) {
        clearInterval(t);
        return;
      }
      if (document.visibilityState !== "hidden")
        void qc.invalidateQueries({ queryKey: agentKeys.directory(scope), exact: true });
    }, AGENT_SETTLE_POLL_MS);
    return () => {
      clearInterval(t);
    };
  }, [active, qc, scope]);
}

function Frame({ children }: { children: React.ReactNode }) {
  return <div className="mx-auto flex w-full max-w-page flex-col gap-4">{children}</div>;
}

function Detail({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="flex gap-2 sm:contents md:flex">
      <dt className="text-muted-foreground">{label}</dt>
      <dd className="min-w-0 truncate">{children}</dd>
    </div>
  );
}

function Crumbs({ title }: { title: string }) {
  return (
    <Breadcrumb aria-label="Breadcrumb">
      <BreadcrumbList className="gap-1 sm:gap-1">
        <BreadcrumbItem>
          <BreadcrumbLink
            render={<Link href="/builds" />}
            className="underline-offset-2 hover:underline"
          >
            {copy.build.breadcrumb}
          </BreadcrumbLink>
        </BreadcrumbItem>
        <BreadcrumbSeparator />
        <BreadcrumbItem className="min-w-0">
          <span aria-current="page" className="truncate text-foreground">
            {title}
          </span>
        </BreadcrumbItem>
      </BreadcrumbList>
    </Breadcrumb>
  );
}
