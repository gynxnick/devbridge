import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/ — options tailored for `tauri dev` / `tauri build`.
export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Keep Rust errors visible in the terminal.
  clearScreen: false,
  server: {
    // Tauri expects a fixed port; fail if it's taken.
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**", "**/crates/**", "**/target/**"] },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    css: false,
  },
});
