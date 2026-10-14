import base from "@nasiko/config/eslint";

export default [
  ...base,
  {
    // The lab's convention, kept by the ported mocks: a `_`-prefixed binding is deliberately unused (a destructured-out
    // field, a placeholder parameter). Only this package's verbatim-ported code relies on it.
    files: ["src/**/*.{ts,tsx}"],
    rules: {
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_", ignoreRestSiblings: true },
      ],
    },
  },
];
