import { defineConfig } from "vite";
import { resolve } from "path";

export default defineConfig({
  // Serve from src/ so window URLs like "capture-window/index.html"
  // resolve correctly in both dev and production.
  root: resolve(__dirname, "src"),

  build: {
    // Tauri targets macOS 13+ (Safari 16+) / Windows 10+ (WebView2).
    // Both support top-level await and all ES2022 features.
    target: "esnext",
    outDir: resolve(__dirname, "dist"),
    emptyOutDir: true,
    rollupOptions: {
      input: {
        capture:     resolve(__dirname, "src/capture-window/index.html"),
        preview:     resolve(__dirname, "src/preview-window/index.html"),
        preferences: resolve(__dirname, "src/preferences/index.html"),
      },
    },
  },

  server: {
    port: 1420,
    strictPort: true,
    host: "localhost",
  },

  // Suppress Vite's own output during tauri dev so Rust logs stay readable.
  clearScreen: false,

  // Expose TAURI_* env vars to the frontend.
  envPrefix: ["VITE_", "TAURI_"],
});
