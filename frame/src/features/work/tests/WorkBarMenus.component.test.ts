import "$styles/global.css";
import { expect, test } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import BarMenuStage from "./BarMenuStage.svelte";

test("the composer stays open while a menu it opened is in use, in its portal too", async () => {
  await page.viewport(1000, 600);
  const screen = await render(BarMenuStage);
  const bar = () => screen.container.querySelector<HTMLElement>(".work-bar")!;
  await screen.getByRole("textbox").click();
  await expect.poll(() => bar().dataset.engaged).toBe("true");
  await screen.getByRole("button", { name: "Model" }).click();
  await expect.element(page.getByRole("button", { name: "Opus" })).toBeVisible();
  await expect.poll(() => bar().dataset.engaged).toBe("true");
  await page.getByRole("button", { name: "Opus" }).click();
  await expect.poll(() => bar().dataset.engaged).toBe("true");
  // Leaving the bar for the canvas lets it go.
  await userEvent.click(document.body, { position: { x: 20, y: 20 } });
  await expect.poll(() => bar().dataset.engaged).toBe("false");
  await screen.unmount();
});

test("blank lines do not keep the composer tall: it rests at its own height when emptied or left", async () => {
  await page.viewport(1000, 600);
  const screen = await render(BarMenuStage);
  const field = screen.getByRole("textbox");
  const area = () => screen.container.querySelector<HTMLTextAreaElement>("textarea")!;
  await field.click();
  const rest = area().getBoundingClientRect().height;
  await userEvent.keyboard("{Shift>}{Enter}{Enter}{Enter}{/Shift}");
  expect(area().getBoundingClientRect().height).toBeGreaterThan(rest + 20);
  await userEvent.keyboard("{Backspace}{Backspace}{Backspace}");
  await expect.poll(() => area().getBoundingClientRect().height).toBe(rest);

  await userEvent.keyboard("{Shift>}{Enter}{Enter}{/Shift}");
  await userEvent.click(document.body, { position: { x: 20, y: 20 } });
  await expect.poll(() => area().value).toBe("");
  await expect.poll(() => area().getBoundingClientRect().height).toBe(rest);
  await screen.unmount();
});

test("a portal selection keeps the composer engaged when WebKit leaves focus on the body", async () => {
  await page.viewport(1000, 600);
  const screen = await render(BarMenuStage, { restoreFocus: false });
  await screen.getByRole("textbox").click();
  await screen.getByRole("button", { name: "Model" }).click();
  await page.getByRole("button", { name: "Opus" }).click();
  await new Promise<void>((done) =>
    requestAnimationFrame(() => requestAnimationFrame(() => done())),
  );
  await expect
    .poll(() => screen.container.querySelector<HTMLElement>(".work-bar")!.dataset.engaged)
    .toBe("true");
  await userEvent.click(document.body, { position: { x: 20, y: 20 } });
  await expect
    .poll(() => screen.container.querySelector<HTMLElement>(".work-bar")!.dataset.engaged)
    .toBe("false");
  await screen.unmount();
});
