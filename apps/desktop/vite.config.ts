import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    strictPort: true,
  },
  test: {
    environment: "happy-dom",
    setupFiles: "./src/test-setup.ts",
  },
});
