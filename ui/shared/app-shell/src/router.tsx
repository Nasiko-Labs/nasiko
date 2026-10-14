import {
  createRootRoute,
  createRoute,
  createRouter,
  Outlet,
  type RouteComponent,
  type RouterHistory,
} from "@tanstack/react-router";
import {
  createElement,
  lazy,
  Suspense,
  useEffect,
  type ComponentType,
  type ReactNode,
} from "react";
import { useQuery } from "@tanstack/react-query";
import { RouterNavigation } from "@nasiko/app-core/router-provider";
import { useRouter } from "@nasiko/app-core/navigation";
import { meQueryOptions, useProductScope } from "@nasiko/data-access/product";
import { Reveal } from "@nasiko/ui/motion";
const loadWorkspaceLayout = () =>
  import("./workspace/WorkspaceLayout").then((m) => ({ default: m.WorkspaceLayout }));
const WorkspaceLayout = lazy(loadWorkspaceLayout);

/**
 * A route component the router can preload (the `preload` it calls from `router.preloadRoute`, as
 * `lazyRouteComponent` provides): `load` is the same import React.lazy uses, so a link's intent fetches the chunk
 * and the click then renders without a Suspense fallback.
 */
function preloadable(component: () => ReactNode, load?: () => Promise<unknown>): RouteComponent {
  if (!load) return component;
  return Object.assign(component, {
    preload: async () => {
      await load();
    },
  });
}
import { WorkspaceLoading } from "./workspace/WorkspaceLoading";
import { NotFoundRoute } from "./root/NotFoundRoute";
import { RouteError } from "./root/RouteError";

export interface AppRoute {
  path: string;
  public?: boolean;
  load?: () => Promise<{ default: ComponentType }>;
  redirect?: string;
  superuserOr?: string;
  /** Edition-owned persistent frame shared by related routes; only its outlet changes. */
  layout?: {
    id: string;
    load: () => Promise<{ default: ComponentType<{ children: ReactNode }> }>;
  };
}

function Redirect({ to }: { to: string }) {
  const router = useRouter();
  useEffect(() => {
    router.replace(to);
  }, [router, to]);
  return null;
}
function Superuser({ to, Page }: { to: string; Page: ComponentType }) {
  const me = useQuery(meQueryOptions(useProductScope()));
  if (!me.data) return <p role="status">Checking access…</p>;
  return me.data.is_superuser ? <Page /> : <Redirect to={to} />;
}

/** Both edition roots provide their own generated, lazy route inventory. */
export function createWorkspaceRouter(routes: readonly AppRoute[], history?: RouterHistory) {
  const root = createRootRoute({
    component: () => (
      <RouterNavigation>
        <Suspense fallback={<WorkspaceLoading />}>
          <Outlet />
        </Suspense>
      </RouterNavigation>
    ),
    notFoundComponent: NotFoundRoute,
    errorComponent: ({ error, reset }) => (
      <RouterNavigation>
        <RouteError
          error={error instanceof Error ? error : new Error("Route failed")}
          reset={reset}
        />
      </RouterNavigation>
    ),
  });
  const workspace = createRoute({
    getParentRoute: () => root,
    id: "workspace",
    component: preloadable(
      () => (
        <WorkspaceLayout>
          {/* Route chunks may suspend on a cold click. Keep the authenticated shell and its
              providers visible while only the page content shows its loading state. */}
          <Suspense fallback={<WorkspaceLoading />}>
            <Outlet />
          </Suspense>
        </WorkspaceLayout>
      ),
      loadWorkspaceLayout,
    ),
  });
  const makeRoute = (route: AppRoute) => {
    const Page = route.load ? lazy(route.load) : NotFoundRoute;
    return createRoute({
      getParentRoute: () =>
        route.public || route.redirect ? root : route.layout ? getLayout(route.layout) : workspace,
      path: route.path.replace(/\[([^\]]+)\]/g, (_match, name: string) => "$" + name),
      component: preloadable(() => {
        const content = route.redirect ? (
          <Redirect to={route.redirect} />
        ) : route.superuserOr ? (
          <Superuser to={route.superuserOr} Page={Page} />
        ) : (
          createElement(Page)
        );
        // Slide the incoming route only; never retain an outgoing screen or wait for an exit.
        // Query/filter changes keep this component mounted and do not replay the entrance.
        return route.public || route.redirect ? (
          content
        ) : (
          <div className="h-full min-h-0 w-full overflow-x-clip" data-route-viewport="">
            <Reveal
              recipe={
                route.layout || route.path.split("/").filter(Boolean).length > 1
                  ? "reveal"
                  : "panel"
              }
              from="end"
              className="h-full min-h-0 w-full"
              data-route-transition=""
            >
              {content}
            </Reveal>
          </div>
        );
      }, route.load),
    });
  };
  const layouts = new Map<string, ReturnType<typeof buildLayout>>();
  function buildLayout(layout: NonNullable<AppRoute["layout"]>) {
    const Layout = lazy(layout.load);
    const parent = createRoute({
      getParentRoute: () => workspace,
      id: `layout-${layout.id}`,
      component: preloadable(
        () => (
          <Layout>
            <Suspense fallback={<WorkspaceLoading />}>
              <Outlet />
            </Suspense>
          </Layout>
        ),
        layout.load,
      ),
    });
    return parent;
  }
  function getLayout(layout: NonNullable<AppRoute["layout"]>) {
    const existing = layouts.get(layout.id);
    if (existing) return existing;
    const parent = buildLayout(layout);
    layouts.set(layout.id, parent);
    return parent;
  }
  const protectedRoutes = routes.filter((r) => !r.public && !r.redirect);
  for (const route of protectedRoutes) if (route.layout) getLayout(route.layout);
  return createRouter({
    routeTree: root.addChildren([
      ...routes.filter((r) => r.public || r.redirect).map(makeRoute),
      workspace.addChildren([
        ...protectedRoutes.filter((r) => !r.layout).map(makeRoute),
        ...[...layouts].map(([id, parent]) =>
          parent.addChildren(protectedRoutes.filter((r) => r.layout?.id === id).map(makeRoute)),
        ),
      ]),
    ]),
    ...(history ? { history } : {}),
    // Links are the navigation port's, which preload on intent through `router.preloadRoute` (navigation.tsx).
    defaultPreload: false,
    scrollRestoration: true,
  });
}
