// Ported from nasiko-ui-lab@1b99bdda common/src/features/onboarding/AgentStep.tsx. Local edits: `"use client"`;
// `meQuery` is `meQueryOptions(scope)`; `Button asChild` + TanStack `Link` -> `ButtonLink`
// (../agents/components/ButtonLink.tsx) with the build's or agent's href; Prettier formatting.
"use client";

/**
 * Step 4: the Deploy page's own Upload, GitHub and Registry tabs. A started deploy stays in the guide (`onStarted`);
 * the background follower toasts when its build finishes (plans/feat-deploy.md eng R8).
 */
import { useQuery } from "@tanstack/react-query";
import { ChevronRight, CircleCheck } from "lucide-react";
import { useState } from "react";
import { Alert, AlertDescription } from "@nasiko/ui/primitives";
import { Skeleton } from "@nasiko/ui/primitives";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@nasiko/ui/primitives";
import { GithubTab } from "../deploy/GithubTab";
import { RegistryTab } from "../deploy/RegistryTab";
import type { DeploySearch } from "../deploy/search";
import { type DeployStarted, UploadTab } from "../deploy/UploadTab";
import { meQueryOptions, useProductScope } from "@nasiko/data-access/product";
import { ButtonLink } from "../agents/components/ButtonLink";
import { copy } from "./copy";
import { StepHeading } from "./parts";

export function AgentStep({
  started,
  onStarted,
  onLeave,
}: {
  started: DeployStarted | null;
  onStarted: (to: DeployStarted) => void;
  /** Following a link out of the guide closes it first. */
  onLeave: () => void;
}) {
  const me = useQuery(meQueryOptions(useProductScope())).data;
  // The Deploy page keeps the GitHub tab's picks in its URL; inside the guide they are local.
  const [search, setSearchState] = useState<DeploySearch>({ method: "github" });
  const setSearch = (patch: Partial<DeploySearch>) => {
    setSearchState((s) => ({ ...s, ...patch }));
  };
  return (
    <div className="flex flex-col gap-6">
      <StepHeading title={copy.agent.title} intro={copy.agent.intro} />
      {started ? (
        <Alert role="status">
          <CircleCheck aria-hidden />
          <AlertDescription className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <span>{copy.agent.started}</span>
            {"buildId" in started ? (
              <ButtonLink
                variant="link"
                size="sm"
                className="h-auto px-0"
                href={`/builds/${encodeURIComponent(started.buildId)}`}
                onClick={onLeave}
              >
                {copy.agent.openBuild}
              </ButtonLink>
            ) : (
              <ButtonLink
                variant="link"
                size="sm"
                className="h-auto px-0"
                href={`/agents/${encodeURIComponent(started.agentId)}`}
                onClick={onLeave}
              >
                {copy.agent.openAgent}
              </ButtonLink>
            )}
          </AlertDescription>
        </Alert>
      ) : !me ? (
        <Skeleton className="h-48 w-full" />
      ) : (
        <Tabs defaultValue="upload" className="gap-4">
          {/* Sticks to the top of the guide's scrolling body (px-6 py-8 md:px-10, so -top-8), as on the Deploy page. */}
          <div className="sticky -top-8 z-20 -mx-6 bg-background px-6 py-2 md:-mx-10 md:px-10">
            <TabsList>
              <TabsTrigger value="upload">{copy.agent.upload}</TabsTrigger>
              <TabsTrigger value="github">{copy.agent.github}</TabsTrigger>
              <TabsTrigger value="registry">{copy.agent.registry}</TabsTrigger>
            </TabsList>
          </div>
          <TabsContent value="upload">
            <UploadTab userId={me.sub} prefill={{}} onStarted={onStarted} />
          </TabsContent>
          <TabsContent value="github">
            <GithubTab search={search} setSearch={setSearch} onStarted={onStarted} />
          </TabsContent>
          <TabsContent value="registry">
            <RegistryTab userId={me.sub} onStarted={onStarted} />
          </TabsContent>
        </Tabs>
      )}
      <div className="flex flex-wrap items-center gap-2 rounded-xl border bg-muted/40 px-4 py-3 text-xs text-muted-foreground">
        <span className="font-medium text-foreground">{copy.agent.lifecycle}</span>
        {copy.agent.stages.map((s, i) => (
          <span key={s} className="flex items-center gap-2">
            {i ? <ChevronRight aria-hidden className="size-3.5" /> : null}
            {s}
          </span>
        ))}
      </div>
    </div>
  );
}
