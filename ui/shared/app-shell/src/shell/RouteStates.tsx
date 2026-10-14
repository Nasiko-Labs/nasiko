"use client";

// Ported from nasiko-ui-lab@1b99bdda common/src/app/shell/RouteStates.tsx. Local edits: `"use client"`; the TanStack `ErrorComponentProps` and `router.invalidate()` become error-boundary props (first written for Next's `error.tsx`) (`{ error, reset }`, oss/app/src/app/error.tsx shape) and `router.refresh()`; `ApiError` is `ProductApiError` (`@nasiko/data-access/product`); `import.meta.env.DEV` -> `process.env.NODE_ENV !== "production"`; imports mapped to `@nasiko/ui/*`.
/**
 * The router's default pending and error UI (plan §8 Phase 3). A route that throws while rendering or loading
 * shows this instead of TanStack's bare error; a 403 reads as "No access", never as a crash.
 */
import { useQueryErrorResetBoundary } from "@tanstack/react-query";
import { useRouter } from "@nasiko/app-core/navigation";
import { useEffect } from "react";
import { PageLoader, StateCard } from "@nasiko/ui/composites";
import { Button } from "@nasiko/ui/primitives";
import { ProductApiError as ApiError } from "@nasiko/data-access/product";
import { copy } from "@nasiko/app-core/shell/copy";

/** The route error-boundary contract (from Next's `error.tsx`): the thrown value and a reset that re-renders the segment. */
export interface RouteErrorProps {
  error: unknown;
  reset: () => void;
}

export function RouteError({ error, reset }: RouteErrorProps) {
  const router = useRouter();
  const queryReset = useQueryErrorResetBoundary();
  // A query error thrown to the boundary stays thrown until its boundary resets.
  useEffect(() => {
    queryReset.reset();
  }, [queryReset]);
  if (error instanceof ApiError && error.isForbidden) {
    return (
      <div className="mx-auto mt-10 max-w-lg px-4">
        <StateCard
          tone="warning"
          title={copy.routeError.noAccess}
          fix={copy.routeError.noAccessBody}
        />
      </div>
    );
  }
  const retry = () => {
    reset();
    router.refresh();
  };
  return (
    <div className="mx-auto mt-10 max-w-lg px-4">
      <StateCard
        tone="error"
        title={copy.routeError.title}
        fix={copy.routeError.body}
        action={
          <Button size="sm" variant="outline" onClick={retry}>
            {copy.routeError.tryAgain}
          </Button>
        }
      >
        {process.env.NODE_ENV !== "production" && error instanceof Error ? (
          <code className="text-xs break-all">{error.message}</code>
        ) : null}
      </StateCard>
    </div>
  );
}

export function RoutePending() {
  return <PageLoader label={copy.routeError.loading} />;
}
