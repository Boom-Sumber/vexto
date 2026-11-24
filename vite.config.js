import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const rootDir = fileURLToPath(new URL(".", import.meta.url));
const pages = [
  "main",
  "settings",
  "new_task",
  "popup",
  "feedback",
  "ocr_overlay",
  "screenshot_overlay",
  "scroll_overlay",
  "scroll_control",
  "pin",
  "preview",
];

export default defineConfig({
  root: resolve(rootDir, "frontend"),
  base: "./",
  plugins: [vue()],
  build: {
    outDir: resolve(rootDir, "ui-dist"),
    emptyOutDir: true,
    target: "chrome120",
    assetsDir: "assets",
    rollupOptions: {
      input: Object.fromEntries(
        pages.map((name) => [name, resolve(rootDir, "frontend", `${name}.html`)]),
      ),
    },
  },
});
