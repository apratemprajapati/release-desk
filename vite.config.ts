import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // Cargo locks files in src-tauri/target on Windows; watching them crashes Vite (EBUSY).
    watch: { ignored: ["**/src-tauri/**"] },
  },
});
