import "@nasiko/app-core/zod-jitless";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { RouterProvider } from "@tanstack/react-router";
import { createWorkspaceRouter } from "@nasiko/app-shell/router";
import { startEarlySession } from "@nasiko/app-shell/workspace/early-session";
import { installStaleBuildDetection } from "@nasiko/app-core/stale-build";
import { RootApplication } from "@nasiko/app-shell/root/RootLayout";
import { EditionRoot } from "./edition-root";
import { EditionMockBoot } from "./mock-boot";
import { APP_ROUTES } from "./routes.gen";
import "@nasiko/app-shell/fonts.css";
import "./app/globals.css";

async function boot() {
  const routes = [...APP_ROUTES];
  if (import.meta.env.NASIKO_HARNESS) {
    const { HARNESS_ROUTES } = await import("./harness-routes");
    routes.push(...HARNESS_ROUTES);
  }
  const router = createWorkspaceRouter(routes);
  // The session check starts now, while the layout's and the route's code download (early-session.ts).
  startEarlySession(router);
  installStaleBuildDetection();
  const element = document.getElementById("root");
  if (!element) throw new Error("Missing application root");
  createRoot(element).render(
    <StrictMode>
      <RootApplication mockBoot={EditionMockBoot}>
        <EditionRoot>
          <RouterProvider router={router} />
        </EditionRoot>
      </RootApplication>
    </StrictMode>,
  );
}
void boot();
