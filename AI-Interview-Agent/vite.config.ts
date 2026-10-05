import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import path from 'path';
import {defineConfig} from 'vite';

export default defineConfig(() => {
  return {
    plugins: [react(), tailwindcss()],
    resolve: {
      alias: {
        '@': path.resolve(__dirname, '.'),
      },
    },
    server: {
      // HMR is disabled in AI Studio via DISABLE_HMR env var.
      // Do not modify—file watching is disabled to prevent flickering during agent edits.
      // NOTE: when running behind server.ts (middlewareMode), the HMR WebSocket
      // server instance is injected at runtime (see server.ts). These defaults
      // only apply to standalone `vite dev` and keep the client from opening
      // a stray WebSocket on the wrong port.
      hmr:
        process.env.DISABLE_HMR === 'true'
          ? false
          : {
              // Reuse the same port/protocol as the page; avoids
              // `WebSocket connection failed` behind proxies/iframes.
              clientPort: undefined,
            },
      // Disable file watching when DISABLE_HMR is true to save CPU during agent edits.
      watch: process.env.DISABLE_HMR === 'true' ? null : {},
    },
  };
});
