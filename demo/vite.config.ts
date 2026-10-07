import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Relative, so one build works wherever nginx serves it: /files/demo/
  // from the file store while it is developed, /demo/ from the image. The
  // routes are in the hash for the same reason (docs/demo.md).
  base: "./",
  build: {
    outDir: process.env.TESSARO_DEMO_OUT ?? "../build/demo",
    emptyOutDir: true,
    assetsDir: "assets",
    chunkSizeWarningLimit: 2048,
    sourcemap: false,
  },
  server: {
    port: 5174,
  },
  test: {
    include: ["src/**/*.test.ts"],
  },
});
