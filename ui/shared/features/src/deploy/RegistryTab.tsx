// Ported from nasiko-ui-lab@1b99bdda common/src/features/deploy/RegistryTab.tsx. Local edits: a server snapshot for the import registry (the page is server-rendered here); a finished import
// navigates with `useRouter().push` from the navigation port (`@nasiko/app-core/navigation`) (`/builds/<id>` or `/agents/<id>`); `Button asChild` +
// `Link` -> `ButtonLink` (../agents/components/ButtonLink.tsx); optional props admit `undefined`
// (exactOptionalPropertyTypes); Prettier formatting.
// Legacy ui/ on development (3aac84f66, c721a11a8): the import's one stage is a `StageList`.
"use client";

/**
 * From a registry (plans/feat-deploy.md §4.3; design review 6, 7; eng review R4). One field; the import is a single
 * long request, so the progress is honest: one live step with elapsed time, then Running, never timer-driven steps.
 * It runs in the upload registry (uploads.ts), so leaving the page keeps it going.
 */
import { useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@nasiko/app-core/navigation";
import { AlertTriangle, Dot } from "lucide-react";
import { useEffect, useId, useState, useSyncExternalStore } from "react";
import { StateCard } from "@nasiko/ui/composites";
import { Alert, AlertDescription, AlertTitle } from "@nasiko/ui/primitives";
import { Button } from "@nasiko/ui/primitives";
import { ButtonLink } from "../agents/components/ButtonLink";
import { Card } from "@nasiko/ui/primitives";
import { Field, FieldDescription, FieldLabel } from "@nasiko/ui/primitives";
import { Input } from "@nasiko/ui/primitives";
import { sendRegistryImport } from "./api";
import { copy } from "./copy";
import { canonicalReference, DEFAULT_REGISTRY, parseReference } from "./registry";
import { fmtElapsed, type Stage } from "./steps";
import { useNow } from "@nasiko/app-core/useNow";
import { ELAPSED_TICK_MS } from "./tuning";
import { openRegistryView } from "./follower";
import { dismissImport, importFor, startImport, subscribeUploads } from "./uploads";
import { StageList } from "./components/StageList";
import type { DeployStarted } from "./UploadTab";

export function RegistryTab({
  userId,
  onStarted,
}: {
  userId: string;
  onStarted?: ((to: DeployStarted) => void) | undefined;
}) {
  const ids = useId();
  const qc = useQueryClient();
  const router = useRouter();
  const state = useSyncExternalStore(
    subscribeUploads,
    () => importFor(userId),
    () => null, // no import runs during a server render
  );
  const [reference, setReference] = useState(state?.reference ?? "");
  const [formatError, setFormatError] = useState(false);
  const importing = state?.phase === "importing";
  const clock = useNow(importing ? ELAPSED_TICK_MS : null);

  // Open: this tab shows an import's outcome itself, so the follower doesn't toast it.
  useEffect(() => openRegistryView(), []);

  // Done: the build page when the import built from source, else the agent page (design review 7). A deploy that didn't
  // start (container_name null, D-7) stays here with a link to the agent.
  useEffect(() => {
    if (state?.phase !== "done" || !state.containerName) return;
    dismissImport(userId);
    if (onStarted)
      onStarted(state.buildId ? { buildId: state.buildId } : { agentId: state.agentId });
    else if (state.buildId) router.push(`/builds/${encodeURIComponent(state.buildId)}`);
    else router.push(`/agents/${encodeURIComponent(state.agentId)}`);
  }, [state, userId, router, onStarted]);

  const err = state?.phase === "failed" ? state.error : null;
  const errText = err ? (typeof err.body === "string" && err.body ? err.body : err.message) : null;
  if (err?.status === 403) {
    return (
      <StateCard
        tone="warning"
        title={copy.registry.disabled}
        action={
          <ButtonLink
            href="/deploy?method=upload"
            size="sm"
            variant="outline"
            className="pointer-coarse:min-h-11"
          >
            {copy.github.useUpload}
          </ButtonLink>
        }
      >
        {copy.registry.disabledHint}
      </StateCard>
    );
  }
  const fieldError = formatError
    ? copy.registry.format
    : err && [400, 409, 422].includes(err.status)
      ? errText
      : null;

  const submit = () => {
    if (!parseReference(reference)) {
      setFormatError(true);
      document.getElementById(`${ids}-ref`)?.focus();
      return;
    }
    setFormatError(false);
    dismissImport(userId);
    void startImport(qc, userId, canonicalReference(reference), sendRegistryImport);
  };

  const stages: Stage[] = [
    { id: "building", label: copy.registry.importing, state: "current" },
    { id: "running", label: copy.steps.running, state: "pending" },
  ];

  return (
    <div className="@container/registry">
      <div className="grid grid-cols-1 gap-4 @[768px]/registry:grid-cols-[minmax(0,3fr)_minmax(0,2fr)] @[768px]/registry:items-start">
        <Card className="gap-4 p-4">
          <form
            noValidate
            className="flex flex-col gap-4"
            onSubmit={(e) => {
              e.preventDefault();
              submit();
            }}
          >
            <Field className="gap-1">
              <FieldLabel htmlFor={`${ids}-ref`}>{copy.registry.reference}</FieldLabel>
              <Input
                id={`${ids}-ref`}
                value={reference}
                readOnly={importing}
                placeholder={copy.registry.placeholder}
                autoComplete="off"
                spellCheck={false}
                className="font-mono"
                onChange={(e) => {
                  setReference(e.target.value);
                  setFormatError(false);
                }}
                aria-invalid={!!fieldError}
                aria-describedby={`${ids}-ref-hint`}
              />
              {fieldError ? (
                <p id={`${ids}-ref-hint`} className="text-xs text-destructive">
                  {fieldError}
                </p>
              ) : (
                <FieldDescription id={`${ids}-ref-hint`} className="text-xs">
                  {copy.registry.hint(DEFAULT_REGISTRY)}
                </FieldDescription>
              )}
            </Field>
            {importing ? (
              <div className="flex flex-col gap-2" data-testid="import-progress">
                <StageList
                  stages={stages}
                  sub={(s) =>
                    s.state === "current"
                      ? `${fmtElapsed(clock - state.startedAt)} · ${copy.registry.usual}`
                      : null
                  }
                />
                <p className="text-xs text-muted-foreground">{copy.registry.keepUsing}</p>
              </div>
            ) : (
              <Button type="submit" className="w-full pointer-coarse:min-h-11">
                {copy.registry.submit}
              </Button>
            )}
            {err && !fieldError ? (
              <Alert className="gap-y-1 border-destructive/30 bg-destructive/5 [&>svg]:text-destructive">
                <AlertTriangle aria-hidden />
                <AlertTitle className="line-clamp-none">{copy.deploy.failed}</AlertTitle>
                <AlertDescription>{errText}</AlertDescription>
              </Alert>
            ) : null}
            {state?.phase === "done" && !state.containerName ? (
              <Alert
                className="gap-y-2 border-warning/40 bg-warning/5 [&>svg]:text-warning"
                data-testid="import-not-running"
              >
                <AlertTriangle aria-hidden />
                <AlertTitle className="line-clamp-none">{copy.registry.notRunning}</AlertTitle>
                <AlertDescription>
                  <ButtonLink
                    href={`/agents/${encodeURIComponent(state.agentId)}`}
                    size="sm"
                    variant="outline"
                    className="pointer-coarse:min-h-11"
                  >
                    {copy.registry.openAgent}
                  </ButtonLink>
                </AlertDescription>
              </Alert>
            ) : null}
          </form>
        </Card>
        <Card className="gap-2 p-4 @max-[768px]/registry:order-first">
          <h2 className="text-sm font-semibold">{copy.registry.needs}</h2>
          <ul className="divide-y">
            {copy.registry.items.map((t) => (
              <li key={t} className="flex gap-3 py-2 text-sm">
                <Dot aria-hidden className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
                {t}
              </li>
            ))}
          </ul>
        </Card>
      </div>
    </div>
  );
}
