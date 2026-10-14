// No lab counterpart: the destination's form of the lab's `<Link to="/optimization" search={(prev) => prev}
// hash="settings">` (OptimizationHubPage's Your settings link, Mechanisms' "Your settings"). Local edits: n/a (new file).
"use client";

import { useSearchParams } from "@nasiko/app-core/navigation";

/** This page with its current search and `#settings`: the jump to Your settings (R1D). */
export function useSettingsHref(): string {
  const qs = useSearchParams().toString();
  return `/optimization${qs ? `?${qs}` : ""}#settings`;
}
