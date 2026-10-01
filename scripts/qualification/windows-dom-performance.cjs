const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const { createRequire } = require("node:module");
const assert = require("node:assert/strict");
const { chromium } = createRequire(path.resolve("frame/package.json"))(
  "playwright",
);

// Standalone renderer fixtures only. No connection to the product, CDP port,
// user profile, network site or privileged IPC. Run from the repository root.
const reference = process.argv[2] || "working-tree";
const output = process.argv[3];
function source(file) {
  return reference === "working-tree"
    ? fs.readFileSync(file, "utf8")
    : execFileSync("git", ["show", `${reference}:${file}`], {
        encoding: "utf8",
        maxBuffer: 4 * 1024 * 1024,
      });
}
const rust = source("crates/zephium-engine/src/host/scripts.rs");
const bootstrap = rust.match(
  /const DISCARD_SAFETY_BOOTSTRAP_JS: &str = r#"([\s\S]*?)"#;/,
)[1];
const cosmetics = source("crates/zephium-engine/src/host/content_style.js");
const digest = (text) =>
  require("node:crypto").createHash("sha256").update(text).digest("hex");

(async () => {
  const browser = await chromium.launch({ headless: true });
  const result = {
    reference,
    browser: browser.version(),
    bootstrap_sha256: digest(bootstrap),
    cosmetics_sha256: digest(cosmetics),
    behavior: [],
  };
  async function fixture() {
    const page = await browser.newPage();
    await page.setContent(
      "<!doctype html><body><main>Useful content</main></body>",
    );
    await page.addScriptTag({ content: bootstrap });
    return page;
  }
  try {
    for (const kind of [
      "clean",
      "dirty-form",
      "closed-shadow-dirty",
      "nested-shadow-dirty",
      "beforeunload",
      "removed-beforeunload",
      "explicit-beforeunload",
      "editable",
      "frame",
      "hook-replaced",
      "shadow-overflow",
    ]) {
      const page = await fixture();
      const mask = await page.evaluate((kind) => {
        if (kind === "dirty-form") {
          const input = document.createElement("input");
          document.body.append(input);
          input.value = "unsaved";
        }
        if (kind === "closed-shadow-dirty" || kind === "nested-shadow-dirty") {
          const host = document.createElement("div");
          document.body.append(host);
          let root = host.attachShadow({ mode: "closed" });
          if (kind === "nested-shadow-dirty") {
            const child = document.createElement("span");
            root.append(child);
            root = child.attachShadow({ mode: "closed" });
          }
          const input = document.createElement("input");
          root.append(input);
          input.value = "unsaved";
        }
        if (kind === "beforeunload") addEventListener("beforeunload", () => {});
        if (kind === "explicit-beforeunload")
          window.addEventListener("beforeunload", () => {});
        if (kind === "removed-beforeunload") {
          const listener = () => {};
          addEventListener("beforeunload", listener);
          removeEventListener("beforeunload", listener);
        }
        if (kind === "editable") {
          const edit = document.createElement("div");
          edit.contentEditable = "true";
          document.body.append(edit);
        }
        if (kind === "frame")
          document.body.append(document.createElement("iframe"));
        if (kind === "hook-replaced")
          Element.prototype.attachShadow = () => null;
        if (kind === "shadow-overflow")
          for (let i = 0; i < 257; i++) {
            const host = document.createElement("div");
            document.body.append(host);
            host.attachShadow({ mode: "closed" });
          }
        return globalThis.__zephium_discard_safety_v1__();
      }, kind);
      if (reference === "working-tree")
        assert.equal(
          mask === 1,
          ["clean", "removed-beforeunload"].includes(kind),
          `${kind}: ${mask}`,
        );
      result.behavior.push({ kind, mask });
      await page.close();
    }
    const retention = await fixture();
    await retention.evaluate(() => {
      globalThis.removedRoots = [];
      for (let i = 0; i < 20; i++) {
        const host = document.createElement("div");
        document.body.append(host);
        const root = host.attachShadow({ mode: "closed" });
        root.innerHTML = "<span>removed component payload</span>".repeat(1000);
        removedRoots.push(new WeakRef(root));
        host.remove();
      }
      const liveHost = document.createElement("div");
      document.body.append(liveHost);
      const liveRoot = liveHost.attachShadow({ mode: "closed" });
      const input = document.createElement("input");
      liveRoot.append(input);
      input.value = "unsaved";
    });
    for (let i = 0; i < 3; i++) await retention.requestGC();
    result.retention = await retention.evaluate(() => ({
      detached_roots_retained: removedRoots.filter((ref) => ref.deref()).length,
      live_dirty_mask_after_gc: globalThis.__zephium_discard_safety_v1__(),
    }));
    assert.notEqual(result.retention.live_dirty_mask_after_gc, 1);
    await retention.close();

    const large = await browser.newPage();
    await large.setContent("<!doctype html><body></body>");
    await large.evaluate(() => {
      globalThis.inspections = { selector_results: 0, walker_steps: 0 };
      const query = Document.prototype.querySelectorAll;
      Document.prototype.querySelectorAll = function (...args) {
        const values = Reflect.apply(query, this, args);
        inspections.selector_results += values.length;
        return values;
      };
      const next = TreeWalker.prototype.nextNode;
      TreeWalker.prototype.nextNode = function () {
        inspections.walker_steps++;
        return Reflect.apply(next, this, []);
      };
    });
    await large.addScriptTag({ content: bootstrap });
    result.large = await large.evaluate(() => {
      document.body.innerHTML = "<div></div>".repeat(100000);
      const samples_ms = [];
      let mask;
      for (let i = 0; i < 15; i++) {
        const start = performance.now();
        mask = globalThis.__zephium_discard_safety_v1__();
        samples_ms.push(performance.now() - start);
      }
      return { nodes: 100000, samples_ms, mask, ...inspections };
    });
    assert.notEqual(result.large.mask, 1);
    await large.close();

    const style = await browser.newPage();
    await style.setContent(
      "<!doctype html><body><main>Useful content</main></body>",
    );
    await style.evaluate(() => {
      globalThis.idleJobs = new Map();
      globalThis.idleNext = 0;
      globalThis.styleSteps = 0;
      globalThis.requestIdleCallback = (callback) => {
        const id = ++idleNext;
        idleJobs.set(id, callback);
        return id;
      };
      globalThis.cancelIdleCallback = (id) => idleJobs.delete(id);
      const next = TreeWalker.prototype.nextNode;
      TreeWalker.prototype.nextNode = function () {
        styleSteps++;
        return Reflect.apply(next, this, []);
      };
    });
    await style.addScriptTag({ content: cosmetics });
    result.cosmetics = await style.evaluate(async () => {
      const api = globalThis.__zephium_content_style_v1__;
      const { token, url } = api.inspect();
      const assertSubscription = api.subscription(
        token,
        url,
        "0000000000000001",
        "a".repeat(64),
        "",
        JSON.stringify([
          [".ad", [".ad"]],
          [".late", [".late"]],
          [".during", [".during"]],
        ]),
        "[]",
      );
      function drain() {
        let callbacks = 0;
        while (idleJobs.size) {
          if (++callbacks > 20000) throw Error("discovery did not settle");
          const [id, callback] = idleJobs.entries().next().value;
          idleJobs.delete(id);
          callback({ didTimeout: true, timeRemaining: () => 10 });
        }
        return callbacks;
      }
      drain();
      styleSteps = 0;
      let parent = document.body;
      for (let i = 0; i < 160; i++) {
        const node = document.createElement("div");
        parent.append(node);
        parent = node;
      }
      parent.className = "ad";
      await Promise.resolve();
      const callbacks = drain();
      const steps = styleSteps;
      const hidden = getComputedStyle(parent).display === "none";
      const late = document.createElement("div");
      document.body.append(late);
      await Promise.resolve();
      drain();
      late.className = "late";
      await Promise.resolve();
      drain();
      let first;
      parent = document.body;
      for (let i = 0; i < 400; i++) {
        const node = document.createElement("div");
        parent.append(node);
        if (!first) first = node;
        parent = node;
      }
      await Promise.resolve();
      const [id, callback] = idleJobs.entries().next().value;
      idleJobs.delete(id);
      callback({ didTimeout: true, timeRemaining: () => 10 });
      first.className = "during";
      await Promise.resolve();
      drain();
      return {
        admitted: assertSubscription,
        steps,
        callbacks,
        hidden,
        late_hidden: getComputedStyle(late).display === "none",
        during_walk_hidden: getComputedStyle(first).display === "none",
      };
    });
    assert.equal(result.cosmetics.admitted, true);
    assert.equal(result.cosmetics.hidden, true);
    assert.equal(result.cosmetics.late_hidden, true);
    assert.equal(result.cosmetics.during_walk_hidden, true);
    await style.close();
    if (
      reference === "working-tree" &&
      process.env.ZEPHIUM_EXPECT_OPTIMIZED === "1"
    ) {
      assert.equal(result.retention.detached_roots_retained, 0);
      assert.equal(result.large.selector_results, 0);
      assert.ok(result.large.walker_steps <= 15 * 4097);
      assert.ok(result.cosmetics.steps <= 162);
    }
    const json = JSON.stringify(result, null, 2) + "\n";
    if (output) fs.writeFileSync(output, json, { flag: "wx" });
    process.stdout.write(json);
  } finally {
    await browser.close();
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
