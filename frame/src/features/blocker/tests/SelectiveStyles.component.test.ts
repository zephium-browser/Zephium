import { afterEach, expect, inject, test, vi } from "vitest";

declare module "vitest" {
  export interface ProvidedContext {
    contentStyleSource: string;
  }
}
type StyleApi = {
  inspect(): { token: string; url: string };
  inspectEncoded(): string;
  reuseSubscription(token: string, url: string, generation: string, fingerprint: string): boolean;
  subscription(
    token: string,
    url: string,
    generation: string,
    fingerprint: string,
    css: string,
    index: string,
    exceptions: string,
  ): boolean;
};
let frame: HTMLIFrameElement | undefined;
afterEach(() => {
  vi.restoreAllMocks();
  frame?.remove();
  frame = undefined;
});

test("exhausted generic budget stops discovery until a replacement policy arrives", async () => {
  const { win, api, token, url, update, display } = await fixture();
  const idle = vi.spyOn(win, "requestIdleCallback");
  const index = JSON.stringify([
    [".ad", Array.from({ length: 2049 }, (_, i) => `.ad[data-slot="${i}"]`)],
  ]);
  expect(api.subscription(token, url, "0000000000000001", "c".repeat(64), "", index, "[]")).toBe(
    true,
  );
  const rules = () => [...win.document.adoptedStyleSheets].flatMap((s) => [...s.cssRules]).length;
  await expect.poll(rules).toBe(2048);
  const scheduled = idle.mock.calls.length;
  const later = win.document.createElement("div");
  later.className = "ad";
  later.id = "late";
  win.document.body.append(later);
  await new Promise<void>((resolve) => {
    win.requestAnimationFrame(() => win.requestAnimationFrame(() => resolve()));
  });
  expect(idle.mock.calls.length).toBe(scheduled);
  expect(rules()).toBe(2048);
  expect(update(2, true)).toBe(true);
  await expect.poll(() => display("#late")).toBe("none");
  expect(rules()).toBe(2);
});

async function fixture() {
  frame = document.createElement("iframe");
  const loaded = new Promise<void>((resolve) => {
    frame!.onload = () => resolve();
  });
  frame.srcdoc = `<div class="ad">Ad</div><div class="except">Keep me</div><main>Useful content</main><script>${inject("contentStyleSource")}</script>`;
  document.body.append(frame);
  await loaded;
  const win = frame.contentWindow!;
  const api = (win as unknown as { __zephium_content_style_v1__: StyleApi })
    .__zephium_content_style_v1__;
  const { token, url } = api.inspect();
  const index = JSON.stringify([
    [".ad", [".ad"]],
    [".except", [".except"]],
    ["#late", ["#late"]],
    ...Array.from({ length: 1000 }, (_, i) => [`.unused-${i}`, [`.unused-${i}`]]),
  ]);
  const update = (generation: number, enabled: boolean) =>
    api.subscription(
      token,
      url,
      generation.toString(16).padStart(16, "0"),
      (enabled ? "a" : "b").repeat(64),
      "",
      enabled ? index : "[]",
      '[".except"]',
    );
  const display = (selector: string) =>
    win.getComputedStyle(win.document.querySelector(selector)!).display;
  return { win, api, token, url, update, display };
}

test("generic rules are installed only for observed tokens, including later DOM changes", async () => {
  const { win, update, display } = await fixture();
  expect(update(1, true)).toBe(true);
  await expect.poll(() => display(".ad")).toBe("none");
  expect(display(".except")).not.toBe("none");
  expect(display("main")).not.toBe("none");
  expect([...win.document.adoptedStyleSheets].flatMap((s) => [...s.cssRules]).length).toBe(1);
  const late = win.document.createElement("div");
  win.document.body.append(late);
  late.id = "late";
  await expect.poll(() => display("#late")).toBe("none");
  expect([...win.document.adoptedStyleSheets].flatMap((s) => [...s.cssRules]).length).toBe(2);
});

test("pause removes subscription rules and stale updates cannot re-enable them", async () => {
  const { win, update, display, api, token, url } = await fixture();
  expect(update(1, true)).toBe(true);
  await expect.poll(() => display(".ad")).toBe("none");
  expect(update(2, false)).toBe(true);
  expect(display(".ad")).not.toBe("none");
  expect(update(1, true)).toBe(false);
  const ad = win.document.createElement("div");
  ad.id = "late";
  win.document.body.append(ad);
  expect(display("#late")).not.toBe("none");
  expect(
    api.subscription(
      "0".repeat(32),
      url,
      "0000000000000003",
      "a".repeat(64),
      "body{display:none}",
      "[]",
      "[]",
    ),
  ).toBe(false);
  expect(
    api.subscription(
      token,
      `${url}#changed`,
      "0000000000000003",
      "a".repeat(64),
      "body{display:none}",
      "[]",
      "[]",
    ),
  ).toBe(false);
  expect(update(3, true)).toBe(true);
  await expect.poll(() => display("#late")).toBe("none");
});

test("unchanged subscription reuses its sheet and rejects removed sheets or stale generations", async () => {
  const { win, update, api, token, url, display } = await fixture();
  expect(update(1, true)).toBe(true);
  await expect.poll(() => display(".ad")).toBe("none");
  const sheet = win.document.adoptedStyleSheets[0];
  expect(JSON.parse(api.inspectEncoded()).subscription).toBe("a".repeat(64));
  expect(api.reuseSubscription(token, url, "0000000000000002", "a".repeat(64))).toBe(true);
  expect(win.document.adoptedStyleSheets[0]).toBe(sheet);
  expect(api.reuseSubscription(token, url, "0000000000000001", "a".repeat(64))).toBe(false);
  expect(api.reuseSubscription(token, url, "invalid", "a".repeat(64))).toBe(false);
  expect(api.reuseSubscription(token, url, "0000000000000003", "b".repeat(64))).toBe(false);
  win.document.adoptedStyleSheets = [];
  expect(JSON.parse(api.inspectEncoded()).subscription).toBeNull();
  expect(api.reuseSubscription(token, url, "0000000000000003", "a".repeat(64))).toBe(false);
  expect(update(3, true)).toBe(true);
  await expect.poll(() => display(".ad")).toBe("none");
});
