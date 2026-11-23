import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// In development, API calls go to `relayscope serve` on its default port.
export default defineConfig({
  plugins: [react()],
  server: { proxy: { "/api": "http://127.0.0.1:8790" } },
});
