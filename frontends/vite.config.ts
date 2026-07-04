import { defineConfig } from "vite";

// Build a single self-contained ES module (`opendbpylot-components.js`) that
// registers the custom elements. Drop it into any page with:
//   <script type="module" src="/opendbpylot-components.js"></script>
export default defineConfig({
  build: {
    lib: {
      entry: "src/index.ts",
      formats: ["es"],
      fileName: () => "opendbpylot-components.js",
    },
    outDir: "dist",
    emptyOutDir: true,
    // Bundle everything (lit, plotly) so the file is standalone.
    rollupOptions: {},
  },
});
