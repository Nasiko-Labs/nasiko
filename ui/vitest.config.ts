import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

const r = (p: string) => fileURLToPath(new URL(p, import.meta.url));

export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: "unit",
          include: [
            "shared/**/src/**/*.test.ts",
            "ee/**/src/**/*.test.ts",
            "tooling/**/*.test.mjs",
            "tooling/**/*.test.ts",
          ],
          // The workspace application packages run in the "web" project (jsdom, router harness).
          exclude: [
            "shared/{app-core,app-shell,features,mocks,testing}/**",
            "ee/features/enterprise/**",
            "ee/app/**",
            "ee/registry/app/**",
            "**/node_modules/**",
          ],
          environment: "node",
        },
      },
      {
        plugins: [react()],
        test: {
          name: "ui",
          include: [
            "shared/{ui,data-access}/src/**/*.test.tsx",
            "ee/features/extensions/src/**/*.test.tsx",
          ],
          environment: "jsdom",
          globals: true,
          setupFiles: [r("./tooling/test/dom-setup.ts")],
        },
      },
      {
        plugins: [react()],
        test: {
          name: "security-ui",
          include: ["tests/security/ui/**/*.test.tsx"],
          environment: "jsdom",
          globals: true,
          setupFiles: [r("./tooling/test/dom-setup.ts")],
        },
      },
      {
        plugins: [react()],
        test: {
          name: "web",
          include: [
            "oss/app/src/**/*.test.{ts,tsx}",
            "shared/{app-core,app-shell,features,mocks,testing}/src/**/*.test.{ts,tsx}",
            "ee/app/src/**/*.test.{ts,tsx}",
            "ee/features/enterprise/src/**/*.test.{ts,tsx}",
            "ee/registry/app/src/**/*.test.{ts,tsx}",
          ],
          environment: "jsdom",
          globals: true,
          setupFiles: [r("./shared/testing/src/unit-setup.ts"), r("./shared/testing/src/setup.ts")],
          // The lab's page tests render a whole product page against MSW (its config: 20 s).
          testTimeout: 20_000,
        },
      },
      {
        test: {
          name: "security",
          include: ["tests/security/**/*.test.ts"],
          environment: "node",
        },
      },
    ],
  },
});
