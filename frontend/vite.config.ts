import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { fileURLToPath, URL } from "node:url";

export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: {
      // Pin key test/runtime packages to the frontend's node_modules so that
      // tests located outside frontend/ (e.g. modules/*/ui/__tests__/) can
      // still resolve them.
      vue: fileURLToPath(new URL("./node_modules/vue", import.meta.url)),
      "@vue/test-utils": fileURLToPath(
        new URL("./node_modules/@vue/test-utils", import.meta.url),
      ),
    },
  },
  server: {
    port: 5173,
    strictPort: true,
    proxy: {
      "/api": {
        target: "http://localhost:3000",
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api/, ""),
      },
    },
    fs: {
      allow: [".."],
    },
  },
  test: {
    include: [
      "src/**/*.{test,spec}.{js,ts}",
      "../modules/**/ui/**/*.{test,spec}.{js,ts}",
    ],
    environment: "jsdom",
    server: {
      deps: {
        inline: ["@vue/test-utils"],
      },
    },
  },
});
