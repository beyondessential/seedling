/// <reference types="vitest/config" />
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const RUST_HTTP = "http://localhost:7894";

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    port: 7890,
    strictPort: true,
    proxy: {
      "/connect": RUST_HTTP,
      "/healthz": RUST_HTTP,
    },
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.test.ts", "src/**/*.test.tsx"],
    globals: true,
    setupFiles: ["src/test/setup.ts"],
    coverage: {
      provider: "v8",
      reporter: ["text", "cobertura"],
      reportsDirectory: "coverage",
      include: ["src/**"],
    },
    server: {
      deps: {
        // @mui/material's ESM build does a bare directory import of
        // `react-transition-group/TransitionGroupContext`, which Node's native
        // ESM resolver rejects. Inlining it routes the import through vite's
        // resolver, which handles directory specifiers.
        inline: ["@mui/material"],
      },
    },
  },
});
