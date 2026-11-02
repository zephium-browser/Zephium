import { beforeEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { PageAssistStatusV1 } from "$shared/ipc/bindings";
import { PROFILE } from "$shared/testing/work-models";
import { installChromeMenu } from "$shared/lib/chrome-menu";
import PanelStage from "./PanelStage.svelte";
import { setTabs, tab } from "./tabs-stub.svelte";
import { SPACE, TAB, person, reply, resetSeq, status, text } from "./fixtures";

const native = vi.hoisted(() => ({ status: null as PageAssistStatusV1 | null }));

vi.mock("$domain/tabs", async () => ({ tabs: (await import("./tabs-stub.svelte")).tabs }));
vi.mock("$domain/surface", () => ({ surface: { open: async () => true } }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { models } = await import("$shared/testing/work-models");
  const { accountFixture } = await import("$shared/testing/account");
  return mockBindings({
    pageAssistStatus: async () => native.status!,
    pageAssistClose: async () => true,
    workModels: async () => {
      const view = models({ keys: { open_ai: "valid" }, lead: "openai/gpt-6-luna" });
      return { ...view, effective: { ...view.effective, page: "openai/gpt-6-luna" } };
    },
    accountGet: async () => ({ account: accountFixture(), fault: null, retry_after: null }),
    settingGet: async () => null,
  });
});

const PROMPT = "Which of these is worth an hour tonight?";
const ANSWER = "Three stories stand out today.";

beforeEach(() => {
  resetSeq();
  setTabs({
    tabs: [tab(TAB, "https://news.ycombinator.com/", "Hacker News")],
    active: TAB,
    space: SPACE,
  });
  window.getSelection()?.removeAllRanges();
});

// The sidebar hosting the panel is `select-none`; the stage stands in for it.
async function open() {
  native.status = status([person(1, PROMPT), reply(2, [text(ANSWER)])]);
  await page.viewport(400, 800);
  const screen = await render(PanelStage, { profile: PROFILE, height: 760 });
  const aside = screen.container.querySelector<HTMLElement>("aside")!;
  aside.style.userSelect = "none";
  aside.style.setProperty("-webkit-user-select", "none");
  await expect.element(screen.getByText(ANSWER)).toBeVisible();
  return screen;
}

const selectable = (element: Element) => getComputedStyle(element).userSelect;

test("the person's prompt and the reply's words select and copy; the chrome around them does not", async () => {
  const screen = await open();

  const bubble = screen.container.querySelector<HTMLElement>(".bubble")!;
  const words = screen.container.querySelector<HTMLElement>("article .text")!;
  expect(selectable(bubble)).toBe("text");
  expect(selectable(words)).toBe("text");

  await userEvent.tripleClick(screen.getByText(PROMPT));
  expect(window.getSelection()?.toString().trim()).toBe(PROMPT);

  await userEvent.tripleClick(screen.getByText(ANSWER));
  expect(window.getSelection()?.toString().trim()).toBe(ANSWER);

  window.getSelection()?.removeAllRanges();
  const chip = screen.container.querySelector<HTMLElement>(".chip")!;
  await userEvent.tripleClick(chip);
  expect(window.getSelection()?.toString().trim()).toBe("");
  const send = screen.container.querySelector<HTMLElement>("button")!;
  expect(selectable(send)).not.toBe("text");
});

test("right-clicking a selection leaves the engine's menu for Copy; anything else stays chrome's", async () => {
  const screen = await open();
  const stop = installChromeMenu();
  try {
    const menu = (target: Element) => {
      const event = new MouseEvent("contextmenu", { bubbles: true, cancelable: true });
      target.dispatchEvent(event);
      return event.defaultPrevented;
    };
    const words = screen.container.querySelector<HTMLElement>("article .text p")!;
    expect(menu(words)).toBe(true);
    await userEvent.tripleClick(screen.getByText(ANSWER));
    expect(menu(words)).toBe(false);
    expect(menu(screen.container.querySelector(".chip")!)).toBe(true);
  } finally {
    stop();
  }
});
