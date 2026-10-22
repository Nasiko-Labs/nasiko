// Browser-owned session scope; seeded only after the authenticated /api/me bootstrap.
// The product tour (legacy ui/ on development, 3aac84f66: dd9fe0f64): the tour host is mounted beside the build
// toasts. Development (1c28b0bb9): the budgets slot's `useNavHidden`.
"use client";

import { skipToken, useQuery, useQueryClient, type QueryClient } from "@tanstack/react-query";
import { usePathname, useRouter, useSearchParams } from "@nasiko/app-core/navigation";
import {
  useCallback,
  useEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import type { Scope } from "@nasiko/data-access";
import {
  createProductQueryClient,
  meQueryOptions,
  ProductApiError,
  productKey,
  ProductQueryProvider,
  useProductScope,
  type Me,
} from "@nasiko/data-access/product";
import { LayoutGroup } from "@nasiko/ui/motion";
import { useSlots } from "@nasiko/app-core/edition/edition-context";
import { AppShell } from "../shell/AppShell";
import { watchOtherTabs, windowLocation } from "../shell/sessionSync";
import { isSignedOutLocally, isSigningOut } from "@nasiko/app-core/session";
import { SignOutContext, type SignOut } from "@nasiko/app-core/sign-out";
import { signOut } from "../shell/signOut";
import { workspaceLoginUrl } from "./login-redirect";
import { useGuideHiddenNav } from "@nasiko/features/onboarding/api";
import { BuildToasts } from "@nasiko/features/deploy/components/BuildToasts";
import { TourHost } from "@nasiko/features/tour/TourHost";
import { copy as deployCopy } from "@nasiko/features/deploy/copy";
import { followedCount, subscribeFollower } from "@nasiko/features/deploy/follower";

export interface AppProvidersProps {
  /** The validated /api/me claims returned by Rust. */
  me: Me;
  scope: Scope;
  /** The browser-readable sidebar preference. */
  initialOpen: boolean | undefined;
  /** `false` turns query retries off: the page-test harness (testing/render-app.tsx), as the lab's `renderApp` did. */
  retry?: boolean | undefined;
  children: ReactNode;
  onIdentityChange?: (() => void) | undefined;
}

const NO_ITEMS: readonly string[] = [];

/** Each client's current 401 reaction (set from an effect, so it never reads render-time state through a ref). */
const reactions = new WeakMap<QueryClient, () => void>();

export function AppProviders({
  me,
  scope,
  initialOpen,
  retry,
  children,
  onIdentityChange,
}: AppProvidersProps) {
  const router = useRouter();
  const pathname = usePathname();
  const params = useSearchParams();
  const query = params.toString();
  const href = query ? `${pathname}?${query}` : pathname;
  const [queryClient] = useState<QueryClient>(() => {
    const client = createProductQueryClient({
      onUnauthorized: () => reactions.get(client)?.(),
      retry,
    });
    // The bootstrap has verified who is signed in: show it now, and let the guard's `useQuery` re-check it (stale).
    client.setQueryData(meQueryOptions(scope).queryKey, me, { updatedAt: 0 });
    return client;
  });
  const lifetime = useRef(0);
  useEffect(() => {
    const generation = ++lifetime.current;
    return () => {
      // StrictMode replays setup/cleanup without unmounting the provider. Only dispose an ended lifetime.
      queueMicrotask(() => {
        // eslint-disable-next-line react-hooks/exhaustive-deps -- the latest value is the point: a newer generation means StrictMode re-ran the setup, so this lifetime has not ended
        if (lifetime.current !== generation) return;
        void queryClient.cancelQueries();
        queryClient.clear();
      });
    };
  }, [queryClient]);
  // The reaction reads the location of the moment a request fails (several fail together, long after the first
  // render), so it is re-registered on every URL change and looked up by the client it belongs to.
  useEffect(() => {
    reactions.set(queryClient, () => {
      // Sign out ends the session on purpose; its own navigation goes to plain /login (eng D6).
      if (isSigningOut()) return;
      const to = workspaceLoginUrl(href, { expired: "true" });
      if (to) router.push(to);
    });
    return () => {
      reactions.delete(queryClient);
    };
  });
  return (
    <>
      <ProductQueryProvider scope={scope} queryClient={queryClient}>
        <LayoutGroup>
          <Guard href={href} expectedUser={me.sub} onIdentityChange={onIdentityChange}>
            <Shell initialOpen={initialOpen}>{children}</Shell>
          </Guard>
        </LayoutGroup>
      </ProductQueryProvider>
    </>
  );
}

/**
 * The lab guard's browser half: after a failed logout this browser stays signed out (review D1), and `me` is
 * re-checked on entry (a 401 with no cached user sends to /login; with one it is the query client's expiry).
 */
function Guard({
  href,
  expectedUser,
  children,
  onIdentityChange,
}: {
  href: string;
  expectedUser: string;
  children: ReactNode;
  onIdentityChange?: (() => void) | undefined;
}) {
  const router = useRouter();
  const scope = useProductScope();
  const me = useQuery(meQueryOptions(scope));
  const barrier = isSignedOutLocally();
  useEffect(() => {
    const to = barrier ? workspaceLoginUrl(href, { signout: "failed" }) : null;
    if (to) router.replace(to);
  }, [barrier, href, router]);
  const gone =
    me.error instanceof ProductApiError && me.error.status === 401 && me.data === undefined;
  useEffect(() => {
    // Not while signing out (its own navigation goes to /login), and never from /login itself (./login-redirect.ts).
    const to = gone && !isSigningOut() ? workspaceLoginUrl(href) : null;
    if (to) router.replace(to);
  }, [gone, href, router]);
  const changed =
    onIdentityChange !== undefined && me.data !== undefined && me.data.sub !== expectedUser;
  useEffect(() => {
    if (changed) onIdentityChange();
  }, [changed, onIdentityChange]);
  if (barrier || gone || changed) return null;
  return children;
}

function Shell({
  initialOpen,
  children,
}: {
  initialOpen: boolean | undefined;
  children: ReactNode;
}) {
  const scope = useProductScope();
  const queryClient = useQueryClient();
  const guideHidden = useGuideHiddenNav();
  // A layer's hidden items (EE: Access control until the org context says `can_read_org`), from its own query.
  const { navHidden, shellSplit, budgets } = useSlots();
  const [feature = "nav-hidden", ...parts] = navHidden?.query.queryKey ?? [];
  const layer = useQuery({
    queryFn: skipToken,
    ...navHidden?.query,
    queryKey: productKey(scope, String(feature), ...parts),
  });
  const layerHidden = navHidden ? navHidden.hidden(layer.data) : NO_ITEMS;
  // Budgets are Enterprise only (development 1c28b0bb9): an OSS build has no item to hide, and EE hides its own while
  // its availability probe is out.
  const budgetsHidden = budgets.useNavHidden();
  // Other tabs' sign-outs and sign-ins (review D2): installed once per client.
  useEffect(
    () =>
      watchOtherTabs({
        queryClient,
        meKey: meQueryOptions(scope).queryKey,
        location: windowLocation,
        recheck: () =>
          void queryClient.invalidateQueries({ queryKey: meQueryOptions(scope).queryKey }),
      }),
    [queryClient, scope],
  );
  // Builds started this session and still running: the Agents item's badge (plans/feat-deploy.md §5).
  const building = useSyncExternalStore(subscribeFollower, followedCount, () => 0);
  // The shell's sign-out for the pages (the Password page after a change that ends the session): features can't
  // import the shell, so it is handed down with this client bound.
  const pageSignOut = useCallback<SignOut>(
    (opts) => signOut({ queryClient, userId: opts.userId, navigate: opts.navigate }),
    [queryClient],
  );
  return (
    <SignOutContext value={pageSignOut}>
      <AppShell
        end={
          <>
            <BuildToasts />
            <TourHost />
          </>
        }
        split={shellSplit}
        hidden={[...guideHidden, ...layerHidden, ...budgetsHidden]}
        badges={
          building
            ? { "/agents": { count: building, label: deployCopy.toast.agentsBadge(building) } }
            : undefined
        }
        initialOpen={initialOpen}
      >
        {children}
      </AppShell>
    </SignOutContext>
  );
}
