import { fileURLToPath } from "node:url";
import type { UserConfig } from "vite";
import { alias } from "./aliases";
import { bootstrapReport, type Page } from "./bootstrap-report";
import { paraglideVitePlugin } from "@inlang/paraglide-js";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";

/**
 * One build of the frame. The browser and the launcher share a graph, so
 * their common code is fetched once. Onboarding is built on its own: sharing
 * a graph with the browser would split code out of the browser's own chunks
 * and cost every launch requests that only a first run needs.
 */
export function frameConfig(pages: readonly Page[]): UserConfig {
  const later = !pages.includes("browser");
  return {
    appType: "mpa",
    resolve: { alias },
    plugins: [
      bootstrapReport(pages),
      paraglideVitePlugin({
        project: "./project.inlang",
        outdir: "./src/shared/i18n",
        emitTsDeclarations: true,
        strategy: ["baseLocale"],
      }),
      svelte(),
      tailwindcss(),
    ],
    clearScreen: false,
    // Scan every source up front. Crawling from the pages alone misses
    // dependencies that only lazy views import, such as the editor; Vite then
    // re-bundles when one is first opened, and the already-open page is left
    // requesting stale modules that answer 504 until it reloads.
    optimizeDeps: {
      entries: [
        "browser.html",
        "panel.html",
        "onboarding.html",
        "src/**/*.svelte",
        "src/**/*.ts",
        "!src/**/*.d.ts",
        "!src/**/tests/**",
      ],
    },
    server: {
      port: 1420,
      strictPort: true,
    },
    // A later build adds to the first one's output instead of replacing it.
    publicDir: later ? false : undefined,
    build: {
      emptyOutDir: !later,
      target: "es2022",
      // Bundled assets have no network latency. Eagerly preloading the whole
      // module graph can overflow the native protocol's 32-request admission
      // limit before the main stylesheet loads. Native module imports schedule
      // their dependencies; Vite still loads CSS before each lazy destination.
      modulePreload: false,
      rolldownOptions: {
        output: {
          // Bound the native custom-protocol startup burst without making lazy
          // tools load the entire initial UI. Translation chunks follow their
          // consumers; only small shared controls and runtimes are consolidated.
          codeSplitting: {
            groups: [
              {
                name: "messages",
                test: /[\\/]src[\\/]shared[\\/]i18n[\\/]/u,
                entriesAware: true,
                entriesAwareMergeThreshold: 0,
              },
              {
                // The icon package ships one module per icon; unbundled, the
                // startup icons would each add a request at launch.
                name: "icons",
                test: /[\\/]@hugeicons[\\/]core-free-icons[\\/]/u,
                tags: ["$initial"],
              },
              {
                name: "ui-core",
                test: /[\\/]src[\\/]shared[\\/]ui[\\/](Icon|Button|FavIcon)[\\/]/u,
                tags: ["$initial"],
              },
              {
                name: "chrome-shared",
                priority: 2,
                // Preferences are read by lazy destinations too; kept here they
                // never make a destination import the browser entry.
                test: /[\\/]src[\\/](?:shared[\\/](ipc[\\/]|platform\.ts|lib[\\/]motion\.ts|i18n[\\/]runtime\.js)|domain[\\/]preferences[\\/])/u,
                tags: ["$initial"],
              },
            ],
          },
        },
        input: Object.fromEntries(
          pages.map((page) => [page, fileURLToPath(new URL(`./${page}.html`, import.meta.url))]),
        ),
      },
    },
  };
}
