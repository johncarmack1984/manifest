import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Uncommon pinned port; strict so a taken port fails loudly instead of
  // silently shifting.
  server: { port: 47303, strictPort: true },
});
