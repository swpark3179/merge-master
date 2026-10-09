import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// Tauri가 devUrl(http://localhost:1420)로 붙는다. 포트가 바뀌면 tauri.conf.json도 함께 바꾼다.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  build: { outDir: "dist", emptyOutDir: true, target: "es2022" },
});
