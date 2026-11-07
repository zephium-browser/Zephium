import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import { ArrowLeft02Icon } from "@hugeicons/core-free-icons";
import IconButton from "../IconButton.svelte";

const props = (extra: object) => ({ icon: ArrowLeft02Icon, label: "Back", ...extra });

test("a right click opens the menu from the button and does not click it", async () => {
  const onclick = vi.fn();
  const onmenu = vi.fn();
  const screen = await render(IconButton, props({ onclick, onmenu }));

  await screen.getByRole("button", { name: "Back" }).click({ button: "right" });

  expect(onmenu).toHaveBeenCalledTimes(1);
  expect(onmenu.mock.calls[0]![0]).toBeInstanceOf(DOMRect);
  expect(onclick).not.toHaveBeenCalled();
});

test("a press held in place opens the menu and the release is not a click", async () => {
  const onclick = vi.fn();
  const onmenu = vi.fn();
  const screen = await render(IconButton, props({ onclick, onmenu }));
  const button = screen.getByRole("button", { name: "Back" }).element() as HTMLButtonElement;
  const box = button.getBoundingClientRect();
  const at = { clientX: box.left + 4, clientY: box.top + 4, bubbles: true, button: 0 };

  button.dispatchEvent(new PointerEvent("pointerdown", at));
  await vi.waitFor(() => expect(onmenu).toHaveBeenCalledTimes(1));
  button.dispatchEvent(new PointerEvent("pointerup", at));
  button.click();
  expect(onclick).not.toHaveBeenCalled();

  // The next press says the last is over: a plain click clicks again.
  button.dispatchEvent(new PointerEvent("pointerdown", at));
  button.dispatchEvent(new PointerEvent("pointerup", at));
  button.click();
  expect(onclick).toHaveBeenCalledTimes(1);
  expect(onmenu).toHaveBeenCalledTimes(1);
});

test("a quick click clicks, and a press that drifts away opens nothing", async () => {
  const onclick = vi.fn();
  const onmenu = vi.fn();
  const screen = await render(IconButton, props({ onclick, onmenu }));
  const target = screen.getByRole("button", { name: "Back" });

  await target.click();
  expect(onclick).toHaveBeenCalledTimes(1);

  const button = target.element() as HTMLButtonElement;
  const box = button.getBoundingClientRect();
  button.dispatchEvent(
    new PointerEvent("pointerdown", {
      clientX: box.left,
      clientY: box.top,
      bubbles: true,
      button: 0,
    }),
  );
  button.dispatchEvent(
    new PointerEvent("pointermove", { clientX: box.left + 30, clientY: box.top, bubbles: true }),
  );
  await new Promise((done) => setTimeout(done, 700));
  expect(onmenu).not.toHaveBeenCalled();
});

test("without a menu the button is only a button", async () => {
  const onclick = vi.fn();
  const screen = await render(IconButton, props({ onclick }));
  const target = screen.getByRole("button", { name: "Back" });

  await target.click({ button: "right" });
  await userEvent.keyboard("{Escape}");
  await target.click();

  expect(onclick).toHaveBeenCalledTimes(1);
});
