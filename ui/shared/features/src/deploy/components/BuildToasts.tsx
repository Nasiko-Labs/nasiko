// Ported from nasiko-ui-lab@1b99bdda common/src/features/deploy/components/BuildToasts.tsx. Local edits: the toast
// actions navigate with `useRouter().push` from the navigation port (`@nasiko/app-core/navigation`) (`/chat?agent=…` through `hrefFor`,
// `/builds/<id>`, `/agents/<id>`) instead of TanStack's `useNavigate`; Prettier formatting.
"use client";

/**
 * The app's one `Toaster` (eng review) and the follower's notifier (plans/feat-deploy.md §5 Background follow): a build
 * that finishes away from its page says so once, with one action. Sonner's own region announces it; pages keep theirs.
 */
import { useRouter } from "@nasiko/app-core/navigation";
import { useEffect } from "react";
import { toast } from "@nasiko/ui/toast";
import { Toaster } from "@nasiko/ui/toast";
import { copy } from "../copy";
import { setNotifier } from "../follower";
import { hrefFor } from "@nasiko/app-core/url-state";

export function BuildToasts() {
  const router = useRouter();
  useEffect(
    () =>
      setNotifier((n) => {
        if (n.kind === "running") {
          toast.success(copy.build.running(n.name), {
            action: {
              label: copy.build.chat,
              onClick: () => {
                router.push(hrefFor("/chat", { agent: n.agentId ?? n.name }));
              },
            },
          });
        } else if (n.kind === "failed") {
          toast.error(copy.toast.failed(n.name), {
            action: {
              label: copy.toast.seeWhy,
              onClick: () => {
                router.push(`/builds/${encodeURIComponent(n.buildId)}`);
              },
            },
          });
        } else {
          toast.warning(copy.toast.notRunning(n.name), {
            action: {
              label: copy.build.openAgent,
              onClick: () => {
                router.push(`/agents/${encodeURIComponent(n.agentId)}`);
              },
            },
          });
        }
      }),
    [router],
  );
  return <Toaster position="bottom-right" />;
}
