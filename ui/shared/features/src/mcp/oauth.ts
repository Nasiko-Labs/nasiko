// Ported from nasiko-ui-lab@1b99bdda common/src/features/mcp/oauth.ts. Local edits: the refresh invalidates the scoped
// feature prefix `mcpKeys.all(scope)` (scope from `useProductScope()`); Prettier formatting.
"use client";

/**
 * The OAuth popup (plans/feat-mcp.md §3, legacy `#applyConnectOutcome`): opens the provider's page in a 600×720
 * window, checks every 500 ms whether it closed, and then refreshes every MCP read. It also refreshes once when this
 * window regains focus: a provider page with `Cross-Origin-Opener-Policy` severs the handle, so `closed` reads true
 * at once (M-6), and the app turns off refetch-on-focus. A blocked popup has only the focus refresh, plus a link to
 * open the page by hand. Both are torn down on unmount (the legacy page leaked its timer).
 */
import { useQueryClient } from "@tanstack/react-query";
import { useProductScope } from "@nasiko/data-access/product";
import { useEffect, useRef } from "react";
import { toast } from "@nasiko/ui/toast";
import { mcpKeys } from "./api";
import { copy } from "./copy";
import { safePopupUrl } from "./logic";
import { POPUP_FEATURES, POPUP_POLL_MS } from "./tuning";

export function useOAuthPopup() {
  const qc = useQueryClient();
  const scope = useProductScope();
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);
  const onFocus = useRef<(() => void) | null>(null);

  const stop = () => {
    if (timer.current) clearInterval(timer.current);
    timer.current = null;
    if (onFocus.current) window.removeEventListener("focus", onFocus.current);
    onFocus.current = null;
  };
  useEffect(() => stop, []);

  const refresh = () => void qc.invalidateQueries({ queryKey: mcpKeys.all(scope) });
  const onFocusOnce = () => {
    onFocus.current = null;
    refresh();
  };

  /** Opens `raw` (https or the mock's `about:` only) and says what happened. */
  return (raw: string | undefined) => {
    stop();
    const url = safePopupUrl(raw);
    if (!url) {
      toast.error(copy.unsafeUrl);
      return;
    }
    const popup = window.open(url, "mcp-oauth", POPUP_FEATURES);
    onFocus.current = onFocusOnce;
    window.addEventListener("focus", onFocusOnce, { once: true });
    if (!popup) {
      toast.warning(copy.popupBlocked, {
        action: { label: copy.openSignIn, onClick: () => window.open(url, "_blank", "noopener") },
      });
      return;
    }
    toast(copy.oauthOpened);
    timer.current = setInterval(() => {
      if (!popup.closed) return;
      if (timer.current) clearInterval(timer.current);
      timer.current = null;
      refresh();
    }, POPUP_POLL_MS);
  };
}
