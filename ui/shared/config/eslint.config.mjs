// Shared flat ESLint config. Architecture rules (dependency direction, forbidden
// imports, server/client boundary) live in tooling/architecture and are
// applied by every package through `nasikoBoundaries`.
import js from "@eslint/js";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import { nasikoBoundaries } from "../../tooling/architecture/eslint-rules.mjs";

export default [
  {
    ignores: [
      "**/node_modules/**",
      "**/.next/**",
      "**/dist/**",
      "**/storybook-static/**",
      "**/coverage/**",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  {
    languageOptions: {
      parserOptions: { projectService: true, tsconfigRootDir: import.meta.dirname + "/../.." },
    },
    rules: {
      "@typescript-eslint/no-unnecessary-condition": "warn",
      "@typescript-eslint/restrict-template-expressions": ["error", { allowNumber: true }],
    },
  },
  {
    files: ["**/*.{ts,tsx}"],
    plugins: { "react-hooks": reactHooks },
    rules: { ...reactHooks.configs["recommended-latest"].rules },
  },
  ...nasikoBoundaries,
  { files: ["**/*.mjs", "**/*.cjs", "**/*.js"], ...tseslint.configs.disableTypeChecked },
];
