import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { defineConfig, loadEnv, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import { THEME_PREPAINT_SCRIPT } from "../ui/src/theme/prepaint.ts";
import { BOOT_GUARD_SCRIPT } from "../app-core/src/stale-build-boot.ts";
import { securityHeaders } from "./security-headers.mjs";
import { precompress } from "./precompress.mjs";

/**
 * Reo.dev product analytics, as each edition root configures it (`Reo.init` options). Release builds only: the
 * loader is a same-origin file, so `script-src 'self'` plus the control plane's Reo hosts (`CSP_EXTRAS` in
 * oss/server/src/main.rs) admit it without an inline-script hash. Development and test builds never load it.
 */
export interface ReoAnalytics {
  clientID: string;
  [option: string]: string | boolean;
}

const reoLoader = (reo: ReoAnalytics) =>
  `(function(){var o=${JSON.stringify(reo)},s=document.createElement("script");` +
  `s.src="https://static.reo.dev/"+o.clientID+"/reo.js";s.defer=true;` +
  `s.onload=function(){window.Reo&&window.Reo.init(o)};document.head.appendChild(s)})();\n`;

/**
 * Shared build policy; each composition root supplies its edition, entry point and route table. A private root
 * passes its own extra routes (`privateRoutes`): this shared, publication-approved file never reads private paths.
 */
export function createViteConfig(
  edition: "oss" | "ee",
  options: { privateRoutes?: readonly { path: string }[]; reo?: ReoAnalytics } = {},
) {
  return defineConfig(({ command, mode }) => {
    const env = { ...loadEnv(mode, process.cwd(), ""), ...process.env };
    const testBuild = env.NASIKO_TEST_BUILD === "1";
    const development = command === "serve" && !process.argv.includes("preview");
    const harness = env.NASIKO_HARNESS === "1" && (development || testBuild);
    const apiMode = env.VITE_NASIKO_API_MODE === "mock" ? "mock" : "live";
    if (apiMode === "mock" && !development && !testBuild)
      throw new Error("Mock mode requires a development server or NASIKO_TEST_BUILD=1");
    if (env.NASIKO_DEPLOYMENT === "central")
      throw new Error(
        "Centralized browser sessions require the Rust gateway integration; no delegation credential may enter the SPA",
      );
    const outDir = testBuild ? (env.NASIKO_TEST_DIST_DIR ?? "dist-test") : "dist";
    if (!/^dist(?:-test(?:-[a-z0-9]+)?)?$/.test(outDir))
      throw new Error("Invalid SPA output directory");
    const upstream = new URL(env.NASIKO_API_UPSTREAM ?? "http://127.0.0.1:8082");
    if (!["http:", "https:"].includes(upstream.protocol) || upstream.username || upstream.password)
      throw new Error("Invalid API upstream");
    const headers = securityHeaders({ development });
    const analytics = !development && !testBuild ? options.reo : undefined;
    const prepaint: Plugin = {
      name: "nasiko-static-policy",
      transformIndexHtml() {
        return analytics
          ? [
              {
                tag: "script",
                attrs: { src: "/nasiko-analytics.js", defer: true },
                injectTo: "head",
              },
            ]
          : [];
      },
      configureServer(server) {
        server.middlewares.use((req, res, next) => {
          if (req.url?.startsWith("/harness/server-identity") && harness) {
            const url = new URL(req.url, "http://localhost");
            if (
              url.searchParams.has("run") &&
              url.searchParams.get("run") !== env.NASIKO_E2E_RUN_ID
            ) {
              res.statusCode = 404;
              res.end();
              return;
            }
            res.setHeader("Content-Type", "application/json");
            res.end(
              JSON.stringify({
                runId: env.NASIKO_E2E_RUN_ID ?? null,
                pid: process.pid,
                nodeEnv: "development",
                harness: true,
                testBuild: false,
                distDir: "vite-dev",
                buildId: null,
              }),
            );
            return;
          }
          if (req.url === "/nasiko-prepaint.js") {
            res.setHeader("Content-Type", "text/javascript");
            res.end(THEME_PREPAINT_SCRIPT);
            return;
          }
          if (req.url === "/nasiko-boot-guard.js") {
            res.setHeader("Content-Type", "text/javascript");
            res.end(BOOT_GUARD_SCRIPT);
            return;
          }
          if (!harness && /^\/harness(?:[/?]|$)/.test(req.url ?? "")) {
            res.statusCode = 404;
            res.end();
            return;
          }
          next();
        });
      },
      generateBundle() {
        const root = fileURLToPath(new URL("../../", import.meta.url));
        const shared = JSON.parse(
          readFileSync(resolve(root, "tooling/routes/routes.json"), "utf8"),
        ) as { routes: { path: string }[] };
        const privateRoutes = options.privateRoutes ?? [];
        const paths = [...shared.routes, ...privateRoutes].map((r) =>
          r.path.replace(/\[([^\]]+)\]/g, "$$$1"),
        );
        paths.push("/foundation");
        this.emitFile({
          type: "asset",
          fileName: "routes.json",
          source: JSON.stringify({ routes: paths }),
        });
        this.emitFile({
          type: "asset",
          fileName: "csp.json",
          source: JSON.stringify({ "script-src": [] }),
        });
        this.emitFile({
          type: "asset",
          fileName: "nasiko-spa-edition",
          source: testBuild ? "test-only" : edition,
        });
        this.emitFile({
          type: "asset",
          fileName: "nasiko-spa.json",
          source: JSON.stringify({
            format: 1,
            edition,
            rendering: "spa",
            testBuild,
            apiMode,
            harness,
          }),
        });
        this.emitFile({
          type: "asset",
          fileName: "nasiko-prepaint.js",
          source: THEME_PREPAINT_SCRIPT,
        });
        this.emitFile({
          type: "asset",
          fileName: "nasiko-boot-guard.js",
          source: BOOT_GUARD_SCRIPT,
        });
        if (analytics)
          this.emitFile({
            type: "asset",
            fileName: "nasiko-analytics.js",
            source: reoLoader(analytics),
          });
        this.emitFile({
          type: "asset",
          fileName: "security-headers.json",
          source: JSON.stringify(securityHeaders(), null, 2),
        });
      },
      async closeBundle() {
        if (development) return;
        // The worker is a test asset, never part of a release.
        if (!testBuild) {
          const { rm } = await import("node:fs/promises");
          await rm(outDir + "/mockServiceWorker.js", { force: true });
        }
        // The control plane serves these variants by Accept-Encoding (oss/server/src/spa.rs); see ./precompress.mjs.
        precompress(outDir);
      },
    };
    return {
      plugins: [react(), prepaint],
      resolve: { alias: { "@": resolve(process.cwd(), "src") } },
      define: {
        "import.meta.env.VITE_NASIKO_API_MODE": JSON.stringify(apiMode),
        "import.meta.env.VITE_NASIKO_ALLOW_MOCK_BUILD": JSON.stringify(
          testBuild ? "true" : "false",
        ),
        "import.meta.env.VITE_NASIKO_WORKSPACE_ID": JSON.stringify(
          env.NASIKO_WORKSPACE_ID ?? "local",
        ),
        "import.meta.env.NASIKO_HARNESS": JSON.stringify(harness),
      },
      server: {
        strictPort: true,
        headers,
        proxy: {
          "/api": { target: upstream.origin, changeOrigin: false },
          "/health": { target: upstream.origin, changeOrigin: false },
        },
      },
      preview: { strictPort: true, headers: securityHeaders() },
      build: {
        outDir,
        sourcemap: testBuild && env.NASIKO_PROFILE_SOURCEMAPS === "1",
        manifest: true,
        chunkSizeWarningLimit: 600,
      },
    };
  });
}
