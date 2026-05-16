import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  base: "./",
  plugins: [react()],
  server: {
    port: 4000,
    strictPort: false
  },
  preview: {
    port: 4000,
    strictPort: false
  }
});
