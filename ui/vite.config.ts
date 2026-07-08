import { fileURLToPath, URL } from "node:url";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The dashboard is served same-origin from the Rust binary in production, so
// requests use a relative /api base. In `vite dev` we proxy /api to the local
// dashboard (default port 7822). Set VITE_FIXTURES=1 to bypass the network
// entirely and run against the synthetic backend.
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
  server: {
    proxy: {
      "/api": {
        target: "http://localhost:7822",
        changeOrigin: true,
      },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
