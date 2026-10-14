import base from "@nasiko/config/eslint";

export default [
  ...base,
  // Verbatim registry snapshots kept for provenance diffs only; never compiled or imported.
  { ignores: ["src/primitives/_registry/**"] },
  {
    // Vendored shadcn base-nova sources (see src/primitives/REGISTRY.json). They
    // are kept byte-close to upstream so re-vendoring stays a diff, not a merge;
    // strict any-tracking is relaxed for them only. Our own code (composites,
    // motion, table, gauge/heatmap) runs the full rule set.
    files: ["src/primitives/**/*.tsx", "src/charts/chart.tsx"],
    rules: {
      "@typescript-eslint/no-unsafe-assignment": "off",
      "@typescript-eslint/no-unsafe-member-access": "off",
      "@typescript-eslint/no-unsafe-argument": "off",
      "@typescript-eslint/restrict-template-expressions": "off",
      "@typescript-eslint/no-unnecessary-type-assertion": "off",
      "@typescript-eslint/no-unnecessary-condition": "off",
      "@typescript-eslint/no-deprecated": "off",
    },
  },
  {
    // The vendored visx chart kit (src/charts-kit/README.md: Bklit UI → nasiko-ui-lab, ADR-0003
    // amendment pending). Kept byte-close to the lab so re-vendoring stays a diff; fidelity of its
    // rendering and animation is mandatory, so:
    // - no exception for the eager `motion` export any more: every kit element is `m.<tag>` under the
    //   app's strict LazyMotion (README "Fidelity rule"; the tooltips were the last, 2026-10-08), so the
    //   base no-restricted-imports rule applies here too and keeps it that way;
    // - no-unnecessary-condition (a warning in the base config) is off: the kit's defensive checks
    //   over `Record<string, unknown>` rows are upstream code, not ours to tighten;
    // - its file-level react-hooks directives come from the lab's React-Compiler-aware config and
    //   are not all needed under this plugin's recommended-latest set, so unused ones are not reported.
    files: ["src/charts-kit/**/*.{ts,tsx}"],
    linterOptions: { reportUnusedDisableDirectives: "off" },
    rules: {
      "@typescript-eslint/no-unnecessary-condition": "off",
    },
  },
];
