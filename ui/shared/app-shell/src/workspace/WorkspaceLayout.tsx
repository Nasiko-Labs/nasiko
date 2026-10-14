import { useEffect, useRef, useState, type ReactNode } from "react";
import { useRouter, usePathname, useSearchParams } from "@nasiko/app-core/navigation";
import { isSignedOutLocally } from "@nasiko/app-core/session";
import { readSidebarCookie } from "@nasiko/app-core/shell/sidebarState";
import { fetchMe, ProductApiError, type Me } from "@nasiko/data-access/product";
import { AppProviders } from "./app-providers";
import { workspaceLoginUrl } from "./login-redirect";
import { RoutePending } from "../shell/RouteStates";
import { AccessFailure } from "./access-failure";
import { takeEarlySession } from "./early-session";

type Session =
  | { kind: "pending" }
  | { kind: "ready"; me: Me; epoch: number }
  | { kind: "signed-out" }
  | { kind: "failed"; forbidden: boolean };
let nextEpoch = 0;

/** Browser session bootstrap. No protected component mounts until Rust has answered /api/me. */
export function WorkspaceLayout({ children }: { children: ReactNode }) {
  const [session, setSession] = useState<Session>({ kind: "pending" });
  const redirecting = useRef(false);
  const router = useRouter();
  const pathname = usePathname();
  const search = useSearchParams().toString();
  useEffect(() => {
    let active = true;
    let request = new AbortController();
    const load = () => {
      request.abort();
      request = new AbortController();
      const signal = request.signal;
      setSession({ kind: "pending" });
      if (isSignedOutLocally()) {
        setSession({ kind: "signed-out" });
        return;
      }
      // The first bootstrap may already be in flight from boot (early-session.ts); later ones always fetch anew.
      void (takeEarlySession() ?? fetchMe({ signal })).then(
        (me) => {
          if (active && !signal.aborted) setSession({ kind: "ready", me, epoch: ++nextEpoch });
        },
        (error: unknown) => {
          if (!active || signal.aborted) return;
          setSession(
            error instanceof ProductApiError && error.status === 401
              ? { kind: "signed-out" }
              : {
                  kind: "failed",
                  forbidden: error instanceof ProductApiError && error.status === 403,
                },
          );
        },
      );
    };
    load();
    window.addEventListener("nasiko:session-refresh", load);
    return () => {
      active = false;
      request.abort();
      window.removeEventListener("nasiko:session-refresh", load);
    };
  }, []);
  useEffect(() => {
    if (session.kind === "pending") redirecting.current = false;
    if (session.kind === "signed-out" && !redirecting.current) {
      // null on /login: this layout can still be painted while the login page loads (./login-redirect.ts).
      const to = workspaceLoginUrl(pathname + (search ? "?" + search : ""));
      if (!to) return;
      redirecting.current = true;
      router.replace(to);
    }
  }, [session.kind, pathname, search, router]);
  if (session.kind === "pending") return <RoutePending />;
  if (session.kind === "signed-out") return null;
  if (session.kind === "failed")
    return <AccessFailure kind={session.forbidden ? "not-a-member" : "delegation-unavailable"} />;
  return (
    <AppProviders
      key={`${session.me.sub}:${session.epoch}`}
      me={session.me}
      scope={{
        workspaceId: (import.meta.env.VITE_NASIKO_WORKSPACE_ID as string | undefined) ?? "local",
        epoch: session.epoch,
      }}
      onIdentityChange={() => {
        window.dispatchEvent(new Event("nasiko:session-refresh"));
      }}
      initialOpen={readSidebarCookie(document.cookie)}
    >
      {children}
    </AppProviders>
  );
}
