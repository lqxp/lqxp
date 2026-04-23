import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { resolve } from "node:path";

export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: {
      "@": resolve(__dirname, "src")
    }
  },
  build: {
    outDir: resolve(__dirname, "../serve.public"),
    emptyOutDir: true
  },
  server: {
    host: "0.0.0.0",
    port: 4173
  }
});
