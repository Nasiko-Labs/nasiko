import base from "./eslint.config.mjs";

export default [
  ...base,
  // Ported lab tests (docs/ui-integration/porting-guide.md §7) keep the lab's test idioms: non-null assertions on
  // elements a previous assertion found, async handlers without awaits (msw resolvers), loosely typed mock bodies.
  // Ported tests also read seed values defensively (`?.`, `??` on data the seed guarantees), which
  // no-unnecessary-condition flags. Tests never ship; production sources keep the strict rules.
  {
    files: ["**/*.test.{ts,tsx}"],
    rules: {
      "@typescript-eslint/no-non-null-assertion": "off",
      "@typescript-eslint/require-await": "off",
      "@typescript-eslint/no-unsafe-assignment": "off",
      "@typescript-eslint/no-unsafe-member-access": "off",
      "@typescript-eslint/no-unsafe-argument": "off",
      "@typescript-eslint/no-unsafe-return": "off",
      "@typescript-eslint/no-unsafe-call": "off",
      "@typescript-eslint/no-invalid-void-type": "off",
      "@typescript-eslint/no-unused-expressions": "off",
      "@typescript-eslint/no-confusing-void-expression": "off",
      "@typescript-eslint/no-unnecessary-condition": "off",
    },
  },
];
