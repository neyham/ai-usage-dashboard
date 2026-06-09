import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Tauri expects a fixed dev-server port and does not handle a cleared screen well.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: false,
    watch: {
      // Don't watch the Rust side from Vite; Tauri/cargo handles it.
      ignored: ["**/src-tauri/**"],
    },
  },
  // Produce assets relative to index.html so Tauri can load them from disk.
  build: {
    target: "es2021",
    minify: "esbuild",
    sourcemap: false,
  },
});
