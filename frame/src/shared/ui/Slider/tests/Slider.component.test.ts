import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import Slider from "../Slider.svelte";

test("slider exposes its value and steps from the keyboard", async () => {
  const onchange = vi.fn();
  const screen = await render(Slider, {
    label: "Zoom",
    value: 2,
    min: 1,
    max: 3,
    step: 0.5,
    format: (value: number) => `${value}x`,
    onchange,
  });
  const track = screen.getByRole("slider", { name: "Zoom" });
  await expect.element(track).toHaveAttribute("aria-valuenow", "2");
  await expect.element(track).toHaveAttribute("aria-valuetext", "2x");
  await userEvent.keyboard(navigator.platform.includes("Mac") ? "{Alt>}{Tab}{/Alt}" : "{Tab}");
  await expect.element(track).toHaveFocus();
  await userEvent.keyboard("{ArrowRight}");
  expect(onchange).toHaveBeenCalledWith(2.5);
  await expect.element(track).toHaveAttribute("aria-valuetext", "2.5x");
  await userEvent.keyboard("{Home}");
  await expect.element(track).toHaveAttribute("aria-valuenow", "1");
});

test("typing a value clamps and quantizes it", async () => {
  const oncommit = vi.fn();
  const screen = await render(Slider, {
    label: "Delay",
    value: 275,
    min: 0,
    max: 1000,
    step: 5,
    oncommit,
  });
  await screen.getByRole("button", { name: "275" }).click();
  const input = screen.getByRole("textbox", { name: "Delay" });
  await expect.element(input).toHaveFocus();
  await userEvent.keyboard("{Control>}a{/Control}1203{Enter}");
  expect(oncommit).toHaveBeenCalledWith(1000);
  await expect
    .element(screen.getByRole("slider", { name: "Delay" }))
    .toHaveAttribute("aria-valuenow", "1000");
});

test("disabled slider is not focusable", async () => {
  const screen = await render(Slider, { label: "Zoom", value: 1, disabled: true });
  const track = screen.getByRole("slider", { name: "Zoom" });
  await expect.element(track).toHaveAttribute("tabindex", "-1");
});

test("clearing the typed value does not reset the slider to zero", async () => {
  const oncommit = vi.fn();
  const screen = await render(Slider, { label: "Tint", value: 40, oncommit });
  await screen.getByRole("button", { name: "40", exact: true }).click();
  await userEvent.keyboard("{Control>}a{/Control}{Backspace}{Enter}");
  await expect
    .element(screen.getByRole("slider", { name: "Tint" }))
    .toHaveAttribute("aria-valuenow", "40");
  expect(oncommit).not.toHaveBeenCalled();
});
