import { resolve } from "node:path";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

// In development, /overlay/<config> serves the overlay page, as `relayscope serve` does.
const overlayRoute: Plugin = {
  name: "overlay-route",
  configureServer(server) {
    server.middlewares.use((req, _res, next) => {
      if (req.url?.startsWith("/overlay/")) req.url = "/overlay.html";
      next();
    });
  },
};

// Two pages: the web app, and the stream overlay with no framework. In development, API
// calls go to `relayscope serve` on its default port.
export default defineConfig({
  plugins: [react(), overlayRoute],
  build: {
    rollupOptions: {
      input: { main: resolve(__dirname, "index.html"), overlay: resolve(__dirname, "overlay.html") },
    },
  },
  server: { proxy: { "/api": "http://127.0.0.1:8790" } },
});
