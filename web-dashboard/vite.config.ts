import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Relative base so /logs/ and /metrics/ both resolve assets under their Kong prefix.
const base = process.env.VITE_BASE_PATH ?? "./";

export default defineConfig({
  base,
  plugins: [react()],
  server: {
    port: 3000,
    proxy: {
      "/api": {
        target: process.env.VITE_API_PROXY || "http://localhost:9100",
        changeOrigin: true,
      },
      "/auth": {
        target: process.env.VITE_API_PROXY || "http://localhost:9100",
        changeOrigin: true,
      },
    },
  },
});
