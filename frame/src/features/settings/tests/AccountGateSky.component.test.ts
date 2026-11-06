import "$styles/global.css";
import { createRawSnippet } from "svelte";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { isDecoded } from "$shared/sky";
import AccountGate from "../components/account/AccountGate.svelte";

/** Each picture's decode, finished when the test says so. */
const decodes: { url: string; finish: () => void }[] = [];
beforeEach(() => {
  vi.stubGlobal(
    "Image",
    class {
      src = "";
      decode() {
        return new Promise<void>((finish) => {
          decodes.push({ url: this.src, finish });
        });
      }
    },
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
});

const children = createRawSnippet(() => ({ render: () => "<p>Sign in</p>" }));
const pictureOf = (element: HTMLElement) =>
  /url\("(.+)"\)/u.exec(element.style.backgroundImage)?.[1] ?? null;

test("the sky is only ever drawn from a decoded picture, through a resize and on the next page", async () => {
  const first = await render(AccountGate, { children });
  first.container.style.cssText = "inline-size:600px;block-size:300px";
  const sky = first.container.querySelector<HTMLElement>(".sky")!;
  const gate = first.container.querySelector<HTMLElement>(".account-gate")!;
  const drawn: { url: string; decoded: boolean }[] = [];
  const watch = new MutationObserver(() => {
    const url = pictureOf(sky);
    if (url) drawn.push({ url, decoded: isDecoded(url) });
  });
  watch.observe(sky, { attributes: true, attributeFilter: ["style"] });

  await expect.poll(() => decodes.length).toBe(1);
  expect(pictureOf(sky)).toBeNull();
  decodes[0]!.finish();
  await expect.poll(() => pictureOf(sky)).toBe(decodes[0]!.url);
  expect(gate.dataset.ready).toBe("true");

  first.container.style.inlineSize = "3000px";
  await expect.poll(() => decodes.length).toBe(2);
  expect(decodes[1]!.url).not.toBe(decodes[0]!.url);
  expect(pictureOf(sky)).toBe(decodes[0]!.url);
  expect(gate.dataset.ready).toBe("true");
  decodes[1]!.finish();
  await expect.poll(() => pictureOf(sky)).toBe(decodes[1]!.url);

  watch.disconnect();
  expect(drawn.length).toBeGreaterThan(0);
  expect(drawn.filter((picture) => !picture.decoded)).toEqual([]);
  first.unmount();

  const next = await render(AccountGate, { children });
  const nextSky = next.container.querySelector<HTMLElement>(".sky")!;
  const firstPaint = pictureOf(nextSky);
  expect(firstPaint).not.toBeNull();
  expect(isDecoded(firstPaint!)).toBe(true);
  expect(next.container.querySelector<HTMLElement>(".account-gate")!.dataset.ready).toBe("true");
});
