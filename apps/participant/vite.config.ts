import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// 개발 중에는 collect-dev-server(기본 8080)로 API·WebSocket을 넘긴다: pnpm dev:server
const server = process.env.MM_SERVER ?? "http://127.0.0.1:8080";

export default defineConfig({
  plugins: [react()],
  base: "/",
  server: {
    port: 5173,
    proxy: {
      "/api": server,
      "/ws": { target: server.replace(/^http/, "ws"), ws: true },
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
});
