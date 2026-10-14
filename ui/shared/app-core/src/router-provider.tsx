import {
  type Router,
  type AnyRoute,
  useBlocker,
  useLocation,
  useMatches,
  useRouter as useTanStackRouter,
} from "@tanstack/react-router";
import { useEffect, useMemo, type ReactNode } from "react";
import {
  LeaveBlockerContext,
  NavigationContext,
  PathParamsContext,
  type LeaveBlocker,
  type LeaveBlockerOptions,
  type Navigation,
} from "./navigation";
import { registerUnsavedEdits } from "./stale-build";

/** The port's blocker in the app: TanStack Router's `useBlocker`, with the lab's LeaveGuard rules. */
function useRouterLeaveBlocker({ when, samePath }: LeaveBlockerOptions): LeaveBlocker {
  const blocker = useBlocker({
    shouldBlockFn: ({ current, next }) =>
      when && next.pathname !== "/login" && !(samePath && next.pathname === current.pathname),
    enableBeforeUnload: () => when,
    withResolver: true,
  });
  // Unsaved edits hold back an automatic reload onto a newer build (./stale-build.ts).
  useEffect(() => (when ? registerUnsavedEdits() : undefined), [when]);
  return {
    blocked: blocker.status === "blocked",
    proceed: () => {
      blocker.proceed?.();
    },
    reset: () => {
      blocker.reset?.();
    },
  };
}

/** The sole production implementation of the navigation port. */
export function RouterNavigation({ children }: { children: ReactNode }) {
  const router = useTanStackRouter<Router<AnyRoute>>();
  const location = useLocation();
  const params = useMatches({
    select: (matches) => (matches.at(-1)?.params ?? {}) as Record<string, string>,
  });
  const navigation = useMemo<Navigation>(
    () => ({
      push: (href, options) => {
        void router.navigate({
          href,
          resetScroll: options?.scroll ?? true,
          ignoreBlocker: options?.ignoreBlocker ?? false,
        });
      },
      replace: (href, options) => {
        void router.navigate({
          href,
          replace: true,
          resetScroll: options?.scroll ?? true,
          ignoreBlocker: options?.ignoreBlocker ?? false,
        });
      },
      back: () => {
        router.history.back();
      },
      forward: () => {
        router.history.forward();
      },
      refresh: () => {
        window.dispatchEvent(new Event("nasiko:session-refresh"));
        void router.invalidate();
      },
      // The route's code (lazy chunks, see app-shell/router.tsx `preload`), not its data: a route's data prefetch is
      // the feature's own (the lab's loader `preload`s, e.g. features/tokenops/prefetch.ts).
      // Only the path decides which route modules load.
      prefetch: (href) => {
        void router
          .preloadRoute({ to: new URL(href, window.location.href).pathname })
          .catch(() => undefined);
      },
    }),
    [router],
  );
  return (
    <NavigationContext
      value={{
        router: navigation,
        pathname: location.pathname,
        search: location.searchStr,
        hash: location.hash,
      }}
    >
      <PathParamsContext value={params}>
        <LeaveBlockerContext value={useRouterLeaveBlocker}>{children}</LeaveBlockerContext>
      </PathParamsContext>
    </NavigationContext>
  );
}
