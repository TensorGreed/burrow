// @ts-check
import { defineConfig } from "astro/config";
import svelte from "@astrojs/svelte";

// See docs/adr/0005-web-stack.md.
//
// Static output, one indexable page per tool, interactivity opt-in per island. There is
// deliberately no adapter and no server runtime: no server exists that could receive a
// user's file.
export default defineConfig({
  output: "static",
  integrations: [svelte()],
  vite: {
    build: {
      // The wasm module dominates the payload, so keep JS chunking predictable, and
      // inline nothing implicitly: we want to see what actually ships.
      target: "es2022",
      assetsInlineLimit: 0,
    },
    worker: {
      format: "es",
    },
  },
});
