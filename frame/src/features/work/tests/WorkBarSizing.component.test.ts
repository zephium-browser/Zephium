import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import WorkBar from "../components/bar/WorkBar.svelte";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({});
});

afterEach(() => vi.restoreAllMocks());

async function open() {
  await page.viewport(1000, 800);
  const onsubmit = vi.fn();
  const screen = await render(WorkBar, { placeholder: "Ask for anything", onsubmit });
  const field = screen.getByRole("textbox");
  const input = field.element() as HTMLTextAreaElement;
  const bar = screen.container.querySelector<HTMLElement>(".work-bar")!;
  bar.style.transition = "none";
  bar.style.inlineSize = "600px";
  return { screen, field, input, bar, onsubmit };
}

test("wrapped text follows the bar's width without another keystroke", async () => {
  const { field, input, bar } = await open();
  await field.fill("A line of text. ".repeat(10));
  await expect.poll(() => input.clientHeight).toBeGreaterThan(34);
  const wideHeight = input.clientHeight;
  bar.style.inlineSize = "320px";
  await expect.poll(() => input.clientHeight).toBeGreaterThan(wideHeight);
  expect(input.clientHeight).toBe(input.scrollHeight);
  bar.style.inlineSize = "600px";
  await expect.poll(() => input.clientHeight).toBe(wideHeight);
});

test.each([false, true])(
  "typing and programmatic drafts grow, cap and shrink (measured fallback: %s)",
  async (fallback) => {
    if (fallback) {
      const supports = CSS.supports.bind(CSS);
      vi.spyOn(CSS, "supports").mockImplementation((...args: string[]) =>
        args[0] === "field-sizing" ? false : supports(args[0]!, args[1]!),
      );
    }
    const { screen, field, input, onsubmit } = await open();
    if (fallback) input.style.setProperty("field-sizing", "fixed");
    await field.fill("Short");
    await expect.poll(() => input.clientHeight).toBe(34);
    await field.fill("A line of text. ".repeat(10));
    await expect.poll(() => input.clientHeight).toBeGreaterThan(34);
    expect(input.clientHeight).toBe(input.scrollHeight);
    await screen.rerender({ value: "A line of text. ".repeat(60) });
    await expect.poll(() => input.clientHeight).toBe(132);
    await userEvent.keyboard("{Control>}{End}{/Control}");
    input.scrollTop = input.scrollHeight;
    const top = input.scrollTop;
    await userEvent.keyboard("x");
    await new Promise((resolve) => requestAnimationFrame(resolve));
    expect(input.clientHeight).toBe(132);
    expect(input.scrollTop).toBe(top);
    await screen.rerender({ value: "Short" });
    await expect.poll(() => input.clientHeight).toBe(34);
    await userEvent.keyboard("{Enter}");
    expect(onsubmit).toHaveBeenCalledOnce();
    await field.fill("");
    await expect.poll(() => input.clientHeight).toBe(34);
  },
);
