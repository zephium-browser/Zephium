import { describe, expect, test, vi } from "vitest";
import { closeOnMiddleClick, isEmptyTabSurface, openOnEmptyMiddleClick } from "../lib/middle-click";

function mouse(button: number, closest: ((selector: string) => object | null) | null): MouseEvent {
  const target = closest === null ? ({} as EventTarget) : ({ closest } as unknown as EventTarget);
  return {
    button,
    target,
    preventDefault: vi.fn(),
  } as unknown as MouseEvent;
}

describe("isEmptyTabSurface", () => {
  test("empty scroller and list chrome count as empty", () => {
    expect(isEmptyTabSurface(mouse(1, () => null))).toBe(true);
  });

  test("a tab row or control is not empty", () => {
    const row = {};
    expect(isEmptyTabSurface(mouse(1, () => row))).toBe(false);
  });

  test("targets without closest are not empty surface", () => {
    expect(isEmptyTabSurface(mouse(1, null))).toBe(false);
  });
});

describe("openOnEmptyMiddleClick", () => {
  test("middle-click on empty space opens a tab and blocks autoscroll", () => {
    const open = vi.fn();
    const handlers = openOnEmptyMiddleClick(open);

    const down = mouse(1, () => null);
    handlers.onmousedown(down);
    expect(down.preventDefault).toHaveBeenCalledOnce();

    const aux = mouse(1, () => null);
    handlers.onauxclick(aux);
    expect(aux.preventDefault).toHaveBeenCalledOnce();
    expect(open).toHaveBeenCalledOnce();
  });

  test("middle-click on a tab row does not open", () => {
    const open = vi.fn();
    const handlers = openOnEmptyMiddleClick(open);
    const row = {};

    handlers.onmousedown(mouse(1, () => row));
    handlers.onauxclick(mouse(1, () => row));
    expect(open).not.toHaveBeenCalled();
  });

  test("left-click is ignored", () => {
    const open = vi.fn();
    const handlers = openOnEmptyMiddleClick(open);
    handlers.onauxclick(mouse(0, () => null));
    expect(open).not.toHaveBeenCalled();
  });
});

describe("closeOnMiddleClick", () => {
  test("only the middle button closes", () => {
    const close = vi.fn();
    const handlers = closeOnMiddleClick(close);
    handlers.onauxclick(mouse(0, () => null));
    expect(close).not.toHaveBeenCalled();
    handlers.onauxclick(mouse(1, () => null));
    expect(close).toHaveBeenCalledOnce();
  });
});
