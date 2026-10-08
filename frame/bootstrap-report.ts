import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import {
  checkBundleBudget,
  GROWTH_ALLOWANCE,
  loadedBeforeOpening,
  type BundleSize,
} from "./bundle-budget.ts";
import type { Plugin } from "vite";

const BUDGETS = new URL("./bundle-budgets.json", import.meta.url);

/** A page native loads into a privileged WebView. */
export type Page = "browser" | "panel" | "onboarding";

/** Inspect emitted graphs: source folders alone cannot guarantee startup isolation. */
export function bootstrapReport(pages: readonly Page[]): Plugin {
  const surfaceStyles = new Map<string, Set<string>>();
  let recording = false;
  return {
    name: "zephium-bootstrap-boundaries",
    apply: "build",
    configResolved(config) {
      recording = config.mode === "budgets";
    },
    generateBundle(_options, bundle) {
      for (const item of Object.values(bundle)) {
        if (item.type !== "chunk") continue;
        for (const id of Object.keys(item.modules)) {
          if (
            /\/src\/(shared\/testing|gallery)\//u.test(id) ||
            /\/frame\/dev\//u.test(id) ||
            (/\/(tests|fixtures)\//u.test(id) && id.includes("/src/")) ||
            /\.test\.[jt]s$/u.test(id)
          )
            this.error(`Development-only module in production: ${id}`);
        }
      }
      const reports: Record<
        string,
        {
          entry: string;
          staticJsBytes: number;
          staticCssBytes: number;
          /** What is budgeted: the whole graph of a page, the addition of a lazy destination. */
          budgeted: BundleSize;
          css: string[];
          modules: string[];
        }
      > = {};
      const bytes = (files: Iterable<string>) => {
        let sum = 0;
        for (const file of files) {
          const item = bundle[file];
          if (item?.type === "chunk") sum += Buffer.byteLength(item.code);
          else if (item?.type === "asset") sum += Buffer.byteLength(item.source);
        }
        return sum;
      };
      const roots: Array<{ name: string; file: string; surface: boolean }> = [];
      for (const name of pages) {
        const root = Object.values(bundle).find(
          (item) =>
            item.type === "chunk" && item.isEntry && item.facadeModuleId?.endsWith(`/${name}.html`),
        );
        if (!root || root.type !== "chunk") this.error(`Missing ${name} entry chunk`);
        roots.push({ name, file: root.fileName, surface: true });
      }
      for (const item of Object.values(bundle)) {
        if (item.type !== "chunk" || !item.isDynamicEntry || !item.facadeModuleId) continue;
        const at = item.facadeModuleId.lastIndexOf("/src/");
        if (at < 0) continue;
        const dependency = item.facadeModuleId.lastIndexOf("/node_modules/");
        roots.push({
          name:
            dependency >= 0
              ? `lazy:dependency/${item.facadeModuleId.slice(dependency + 14)}`
              : `lazy:${item.facadeModuleId.slice(at + 5)}`,
          file: item.fileName,
          surface: false,
        });
      }
      const panel = roots.find((root) => root.name === "panel");
      // The launcher's WebView is resident all day. Anything it can load
      // at all, not just at startup, is paid again on top of the browser's
      // copy, so an editor or a tool view must be unreachable from it.
      const reachable = new Set<string>();
      const reach = (file: string) => {
        if (reachable.has(file)) return;
        reachable.add(file);
        const item = bundle[file];
        if (!item || item.type !== "chunk") return;
        for (const id of Object.keys(item.modules))
          if (
            /\/(?:@tiptap|prosemirror-[a-z]+|@xyflow|layerchart)\//u.test(id) ||
            /\/src\/features\/(?:notes|tools|work|history|downloads)\//u.test(id)
          )
            this.error(`Panel can load ${id}, which the browser already hosts`);
        for (const child of [...item.imports, ...item.dynamicImports]) reach(child);
      };
      if (panel) reach(panel.file);
      // A first run has its own page and build; nothing of it may be loadable
      // from the browser, eagerly or lazily, so no later launch carries it.
      const browser = roots.find((root) => root.name === "browser");
      const fromBrowser = new Set<string>();
      const follow = (file: string) => {
        if (fromBrowser.has(file)) return;
        fromBrowser.add(file);
        const item = bundle[file];
        if (!item || item.type !== "chunk") return;
        for (const id of Object.keys(item.modules))
          if (/\/src\/(?:features|app)\/onboarding\//u.test(id))
            this.error(`Browser can load onboarding code: ${id}`);
        for (const child of [...item.imports, ...item.dynamicImports]) follow(child);
      };
      if (browser) follow(browser.file);
      const graphs = new Map<string, { visited: Set<string>; css: Set<string> }>();
      for (const root of roots) {
        const name = root.name;
        const visited = new Set<string>();
        const modules = new Set<string>();
        const css = new Set<string>();
        const walk = (file: string) => {
          if (visited.has(file)) return;
          visited.add(file);
          const item = bundle[file];
          if (!item || item.type !== "chunk") return;
          for (const id of Object.keys(item.modules)) {
            if (
              root.surface &&
              (/\/(?:features|domain)\/work\//u.test(id) || /\/(?:@xyflow|@tiptap)\//u.test(id))
            )
              this.error(`Work code in ${name} startup: ${id}`);
            const at = id.indexOf("/src/");
            if (at >= 0) modules.add(id.slice(at + 1));
          }
          for (const child of item.imports) walk(child);
          for (const style of (
            item as typeof item & { viteMetadata?: { importedCss: Set<string> } }
          ).viteMetadata?.importedCss ?? [])
            css.add(style);
        };
        walk(root.file);
        if (root.surface && visited.size + css.size > 24) {
          this.error(
            `${name} startup requests exceed the native asset budget: ${visited.size + css.size} (limit 24)`,
          );
        }
        if (root.surface) surfaceStyles.set(name, css);
        const forbidden = [...modules].filter(
          (id) =>
            /^src\/features\/(sidebar|tabs|essentials|address|spaces|extensions|blocker|settings)\//u.test(
              id,
            ) ||
            /^src\/features\/tools\/components\/(previews\/|ToolSlot\.svelte|ToolFrame\.svelte)/u.test(
              id,
            ) ||
            /^src\/domain\/(tabs|blocker|extensions|runtime|permissions|capture)\//u.test(id) ||
            [
              "src/app/browser/BrowserApp.svelte",
              "src/styles/global.css",
              "src/styles/browser.css",
              "src/session/tool-drafts.svelte.ts",
            ].includes(id),
        );
        if (name === "panel" && forbidden.length)
          this.error(`Panel eagerly loads browser/tool code: ${forbidden.join(", ")}`);

        graphs.set(name, { visited, css });
        const js = bytes(visited);
        const styles = bytes(css);
        reports[name] = {
          entry: root.file,
          staticJsBytes: js,
          staticCssBytes: styles,
          budgeted: { js, css: styles },
          css: [...css],
          modules: [...modules].sort(),
        };
      }
      // A lazy destination costs only what is not already loaded when it
      // opens: its page, and every destination that must have run to open
      // it. Charging it for code its opener already brought would count a
      // bundler placing shared code with that opener as growth.
      const files = (name: string) => {
        const graph = graphs.get(name);
        return new Set([...(graph?.visited ?? []), ...(graph?.css ?? [])]);
      };
      const before = loadedBeforeOpening(
        Object.fromEntries(
          Object.entries(bundle).flatMap(([file, item]) =>
            item.type === "chunk"
              ? [[file, { imports: item.imports, dynamicImports: item.dynamicImports }]]
              : [],
          ),
        ),
        roots.map((root) => ({
          ...root,
          files: files(root.name),
          visited: graphs.get(root.name)?.visited ?? new Set(),
        })),
      );
      for (const root of roots) {
        const graph = graphs.get(root.name);
        const report = reports[root.name];
        if (root.surface || !graph || !report) continue;
        const loaded = before.get(root.name) ?? new Set();
        report.budgeted = {
          js: bytes([...graph.visited].filter((file) => !loaded.has(file))),
          css: bytes([...graph.css].filter((file) => !loaded.has(file))),
        };
      }
      const build = pages.join("-");
      const recordedBuilds = JSON.parse(readFileSync(BUDGETS, "utf8")) as Record<
        string,
        Record<string, BundleSize>
      >;
      if (recording) {
        recordedBuilds[build] = Object.fromEntries(
          Object.entries(reports)
            .sort(([a], [b]) => a.localeCompare(b))
            .map(([name, report]) => [name, report.budgeted]),
        );
        writeFileSync(BUDGETS, `${JSON.stringify(recordedBuilds, null, 2)}\n`);
      } else {
        const recorded = recordedBuilds[build] ?? {};
        const failures: string[] = [];
        for (const [name, report] of Object.entries(reports)) {
          const size = Object.hasOwn(recorded, name) ? recorded[name] : undefined;
          const failure = checkBundleBudget(name, report.budgeted, size);
          if (failure) failures.push(failure);
          else if (
            size &&
            (size.js - report.budgeted.js > GROWTH_ALLOWANCE.js ||
              size.css - report.budgeted.css > GROWTH_ALLOWANCE.css)
          )
            this.warn(`${name} is well under its recorded size; run pnpm run budgets to lower it`);
        }
        if (failures.length)
          this.error(
            `${failures.join("\n")}\nShrink it, or if the growth is intended, run pnpm run budgets and say why in the commit.`,
          );
      }
      // Kept out of dist: Tauri embeds all of dist into the shipped binary.
      const dir = new URL("./reports/", import.meta.url);
      mkdirSync(dir, { recursive: true });
      writeFileSync(
        new URL(
          pages.includes("browser")
            ? "bootstrap-report.json"
            : `bootstrap-report.${pages.join("-")}.json`,
          dir,
        ),
        JSON.stringify(reports, null, 2),
      );
    },
    writeBundle(_options, bundle) {
      for (const [name, styles] of surfaceStyles) {
        const html = bundle[`${name}.html`];
        if (!html || html.type !== "asset") this.error(`Missing ${name} HTML`);
        // Tauri nonce-injects inline style tags. That makes unsafe-inline
        // ineffective and prevents dropdowns from restoring body input styles.
        if (/<style[\s>]/iu.test(String(html.source)))
          this.error(
            `${name} contains inline CSS; startup paint must stay in external stylesheets`,
          );
        for (const style of styles) {
          if (!String(html.source).includes(style))
            this.error(`${name} stylesheet missing from its HTML preload graph: ${style}`);
        }
      }
    },
  };
}
