// One class-merge helper for the whole foundation: shadcn's `cn` package (MIT; a tailwind-merge-compatible engine
// with clsx arguments), configured like the lab's `cn` (nasiko-ui-lab@1b99bdda common/src/lib/utils.ts): the merge
// engine only knows Tailwind's default scales, so without this theme's names (`max-w-page`, `max-w-sheet*`,
// `text-code`, `text-lead`) a sheet's `sm:max-w-sheet-lg` lost to the sheet primitive's `sm:max-w-sm` (found by the
// full-migration fidelity run: the router's config sheet drew 384 px wide instead of 720 px), and `text-code` /
// `text-lead` read as colours. Every foundation file merges through this one (check:architecture pins "cn" to
// @nasiko/ui), and application code imports it from "@nasiko/ui/cn".
import type { CnFunction } from "cn";
import { createCn } from "cn/config";

export const cn: CnFunction = createCn({
  extend: {
    theme: {
      container: ["page", "sheet", "sheet-sm", "sheet-lg"],
      text: ["code", "lead"],
    },
  },
});
