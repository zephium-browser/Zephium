import "$styles/global.css";
import { createRawSnippet } from "svelte";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { skyImage, type Scene } from "$shared/sky";
import AccountGate from "../components/account/AccountGate.svelte";

const decodes = vi.hoisted(() => [] as { width: number | undefined; finish: () => void }[]);
vi.mock("$shared/sky", async (original) => {
  const real = await original<typeof import("$shared/sky")>();
  return {
    ...real,
    isDecoded: () => false,
    decode: (_scene: Scene, width?: number) =>
      new Promise<void>((finish) => {
        decodes.push({ width, finish });
      }),
  };
});

test("a resize to a wider picture keeps the decoded sky on show until the wider one has decoded", async () => {
  const screen = await render(AccountGate, {
    children: createRawSnippet(() => ({ render: () => "<p>Sign in</p>" })),
  });
  screen.container.style.cssText = "inline-size:600px;block-size:300px";
  const gate = () => screen.container.querySelector<HTMLElement>(".account-gate")!;
  const sky = () => screen.container.querySelector<HTMLElement>(".sky")!.style.backgroundImage;
  const picture = (width: number | undefined) =>
    `url("${skyImage(gate().dataset.sky as Scene, width ?? 0)}")`;

  await expect.poll(() => decodes.length).toBe(1);
  expect(decodes[0]!.width).toBeGreaterThan(0);
  decodes[0]!.finish();
  await expect.poll(() => gate().dataset.ready).toBe("true");
  const narrow = sky();
  expect(narrow).toBe(picture(decodes[0]!.width));

  screen.container.style.inlineSize = "3000px";
  await expect.poll(() => decodes.length).toBe(2);
  expect(picture(decodes[1]!.width)).not.toBe(narrow);
  expect(sky()).toBe(narrow);
  expect(gate().dataset.ready).toBe("true");

  decodes[1]!.finish();
  await expect.poll(() => sky()).toBe(picture(decodes[1]!.width));
});
