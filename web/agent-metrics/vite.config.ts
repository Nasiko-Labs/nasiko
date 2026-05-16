import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  base: "/metrics/",
  server: {
    port: 4010,
    proxy: {
      "/api": "http://localhost:9100",
      "/auth": "http://localhost:9100",
    },
  },
});
