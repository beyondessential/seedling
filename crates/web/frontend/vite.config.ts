import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

const RUST_HTTP = "http://localhost:8080";

export default defineConfig({
  plugins: [react()],
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
  server: {
    proxy: {
      "/connect": RUST_HTTP,
      "/healthz": RUST_HTTP,
    },
  },
});
