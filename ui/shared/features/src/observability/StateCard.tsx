// Ported from nasiko-ui-lab@1b99bdda common/src/features/observability/StateCard.tsx. Local edits: `StateCard` and
// `Button` come from `@nasiko/ui/composites` / `@nasiko/ui/primitives`; the lab's `ApiError` is `ProductApiError`
// (`@nasiko/data-access/product`); `fix` passes `undefined` explicitly where the lab omitted it
// (exactOptionalPropertyTypes); Prettier formatting.
"use client";

/**
 * Observability's error mapping onto the shared `StateCard` (components/shared/state-card): what
 * happened, then what to do about it. Never a bare "No items found".
 */
import { RotateCw } from "lucide-react";
import { StateCard } from "@nasiko/ui/composites";
import { Button } from "@nasiko/ui/primitives";
import { ProductApiError as ApiError } from "@nasiko/data-access/product";
import { copy } from "./copy";

export { StateCard };

/** Map an observability error to its state (503 not configured, 500/502 store error, 403 no access, 404 neutral, proxy down). */
export function ErrorState({
  error,
  onRetry,
  notFound = copy.notFound,
}: {
  error: unknown;
  onRetry: () => void;
  notFound?: string | undefined;
}) {
  const api = error instanceof ApiError ? error : null;
  const retry = (
    <Button size="sm" variant="outline" onClick={onRetry}>
      <RotateCw className="size-3.5" aria-hidden /> Retry
    </Button>
  );
  if (api?.isServerUnreachable && api.status === 502 && !api.serverMessage) {
    return (
      <StateCard
        tone="error"
        title="Can't reach nasiko-server"
        fix={copy.serverDown("NASIKO_API_URL")}
        action={retry}
      />
    );
  }
  if (api?.isForbidden)
    return <StateCard tone="warning" title={copy.noAccess} fix={copy.noAccessFix} />;
  if (api?.status === 503)
    return (
      <StateCard
        tone="warning"
        title={copy.notConfigured}
        fix={copy.notConfiguredFix}
        action={retry}
      />
    );
  if (api?.status === 404) return <StateCard title={notFound} />;
  if (api && api.status >= 500)
    return (
      <StateCard
        tone="error"
        title={copy.traceStoreError}
        fix={api.serverMessage ? `Server said: ${api.serverMessage}` : undefined}
        action={retry}
      />
    );
  return (
    <StateCard
      tone="error"
      title="Something went wrong"
      fix={api?.serverMessage ?? (error instanceof Error ? error.message : undefined)}
      action={retry}
    />
  );
}
