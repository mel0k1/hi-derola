import path from "node:path";
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
      cn: path.resolve(__dirname, "./src/lib/utils"),
    },
  },
  server: {
    port: 5183,
    strictPort: true,
  },
  build: {
    outDir: "dist",
    chunkSizeWarningLimit: 1200,
  },
});
