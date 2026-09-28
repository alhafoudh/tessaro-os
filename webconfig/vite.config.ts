import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { defineConfig } from "vitest/config";

import { cookieToBrowser, cookieToDevice, originOf } from "./dev-proxy";

// The device `webconfig:run` proxies the API to: the qemu forward by
// default (docs/e2e.md).
const target = process.env.TESSARO_WEBCONFIG_TARGET ?? "https://127.0.0.1:17400";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  build: {
    // Where the agent's integration task and the recipe expect it; both
    // serve it as the device's `/`.
    outDir: process.env.TESSARO_WEBCONFIG_OUT ?? "../build/webconfig",
    emptyOutDir: true,
    assetsDir: "assets",
    // Everything the device serves is under 8 MB a file (statics.rs).
    chunkSizeWarningLimit: 2048,
    sourcemap: false,
  },
  server: {
    port: 5173,
    proxy: {
      "/api": {
        target,
        // The device's certificate is self-signed.
        secure: false,
        changeOrigin: true,
        configure(proxy) {
          proxy.on("proxyReq", (request) => {
            if (request.getHeader("origin")) {
              request.setHeader("origin", originOf(target));
            }
            const cookie = request.getHeader("cookie");
            if (typeof cookie === "string") {
              request.setHeader("cookie", cookieToDevice(cookie));
            }
          });
          proxy.on("proxyRes", (response) => {
            const cookies = response.headers["set-cookie"];
            if (cookies) {
              response.headers["set-cookie"] = cookies.map(cookieToBrowser);
            }
          });
        },
      },
    },
  },
  test: {
    include: ["src/**/*.test.ts", "*.test.ts"],
  },
});
