import { createViteConfig } from "@nasiko/config/vite";

// Reo.dev analytics, as the legacy OSS index.html loaded it (release builds only; see createViteConfig).
export default createViteConfig("oss", {
  reo: { clientID: "f4cf7633edc0a46", source: "internal" },
});
