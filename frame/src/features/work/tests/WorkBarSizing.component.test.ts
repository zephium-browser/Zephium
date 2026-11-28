import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import { IS_MAC } from "$shared/platform";
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

/** Everything a person could see move when a letter is typed. */
function geometry(input: HTMLTextAreaElement) {
  const rect = input.getBoundingClientRect();
  return {
    height: rect.height,
    width: rect.width,
    top: rect.top,
    bottom: rect.bottom,
    clientWidth: input.clientWidth,
  };
}

test("wrapped text follows the bar's width without another keystroke", async () => {
  const { field, input, bar } = await open();
  await field.fill("A line of text. ".repeat(10));
  await expect.poll(() => input.clientHeight).toBeGreaterThan(34);
  const wideHeight = input.clientHeight;
  bar.style.inlineSize = "320px";
  await expect.poll(() => input.clientHeight).toBeGreaterThan(wideHeight);
  bar.style.inlineSize = "600px";
  await expect.poll(() => input.clientHeight).toBe(wideHeight);
});

test.each([false, true])(
  "typing never moves the field, at one line or at the cap (measured fallback: %s)",
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
    const rest = geometry(input);

    // Words that stay on the first line cannot shift the field at all.
    await userEvent.keyboard(" words that stay here");
    expect(input.value).toBe("Short words that stay here");
    expect(geometry(input)).toEqual(rest);

    // Nor can letters added to a draft already tall enough to scroll.
    const draft = "A short line\n".repeat(12) + "Last line";
    await screen.rerender({ value: draft });
    await expect.poll(() => input.clientHeight).toBe(132);
    await userEvent.keyboard(IS_MAC ? "{Meta>}{ArrowDown}{/Meta}" : "{Control>}{End}{/Control}");
    expect(input.selectionStart).toBe(draft.length);
    input.scrollTop = input.scrollHeight;
    const tall = geometry(input);
    const scrollTop = input.scrollTop;
    await userEvent.keyboard("x");
    await new Promise((resolve) => requestAnimationFrame(resolve));
    expect(input.value).toBe(`${draft}x`);
    expect(geometry(input)).toEqual(tall);
    expect(input.scrollTop).toBe(scrollTop);

    // The scrollbar's gutter is reserved in both states, so crossing the cap
    // cannot narrow the text and reflow it under the caret.
    expect(input.clientWidth).toBe(rest.clientWidth);
    await screen.rerender({ value: "Short" });
    await expect.poll(() => input.clientHeight).toBe(34);
    await userEvent.keyboard("{Enter}");
    expect(onsubmit).toHaveBeenCalledOnce();
    await field.fill("");
    await expect.poll(() => input.clientHeight).toBe(34);
  },
);
