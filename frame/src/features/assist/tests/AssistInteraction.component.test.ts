import { beforeEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { PageAssistPickResponseV1, PageAssistStatusV1 } from "$shared/ipc/bindings";
import { PROFILE } from "$shared/testing/work-models";
import { emitNativeEvent } from "$shared/testing/native-events";
import PanelStage from "./PanelStage.svelte";
import { setTabs, tab } from "./tabs-stub.svelte";
import {
  CONVERSATION,
  HANDOFF,
  ITEMS,
  SPACE,
  TAB,
  event,
  person,
  reply,
  resetSeq,
  status,
  text,
} from "./fixtures";

const native = vi.hoisted(() => ({
  status: null as PageAssistStatusV1 | null,
  statusCalls: 0,
  ask: vi.fn(async () => ({ reply: 2, fault: null })),
  stop: vi.fn(async () => true),
  focus: vi.fn(async () => true),
  decide: vi.fn(async () => true),
  continueWork: vi.fn(async () => ({ work: "w1", fault: null })),
  pick: vi.fn(async (): Promise<PageAssistPickResponseV1> => ({ pick: null, fault: null })),
  pickCancel: vi.fn(async () => true),
  detach: vi.fn(async () => true),
  close: vi.fn(async () => true),
  history: vi.fn(async () => ({
    version: 2,
    conversations: [
      {
        id: "c0",
        title: "Pricing on the Pro plan",
        space: "space-1",
        updated_ms: Date.now() - 3_600_000,
      },
    ],
  })),
  open: vi.fn(async () => true),
  remove: vi.fn(async () => true),
  url: vi.fn(async () => ({ accepted: true, operation_id: null })),
  surface: vi.fn(async () => true),
}));

vi.mock("$domain/tabs", async () => ({ tabs: (await import("./tabs-stub.svelte")).tabs }));
vi.mock("$domain/surface", () => ({ surface: { open: native.surface } }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const { models } = await import("$shared/testing/work-models");
  const { accountFixture } = await import("$shared/testing/account");
  return mockBindings({
    pageAssistStatus: async () => {
      native.statusCalls++;
      return native.status!;
    },
    pageAssistAsk: native.ask,
    pageAssistStop: native.stop,
    pageAssistFocus: native.focus,
    pageAssistDecide: native.decide,
    pageAssistContinue: native.continueWork,
    pageAssistPick: native.pick,
    pageAssistPickCancel: native.pickCancel,
    pageAssistDetach: native.detach,
    pageAssistClose: native.close,
    pageAssistHistory: native.history,
    pageAssistOpen: native.open,
    pageAssistDelete: native.remove,
    browserOpenUrl: native.url,
    workModels: async () => {
      const view = models({ keys: { open_ai: "valid" }, lead: "openai/gpt-6-luna" });
      return { ...view, effective: { ...view.effective, page: "openai/gpt-6-luna" } };
    },
    accountGet: async () => ({ account: accountFixture(), fault: null, retry_after: null }),
    settingGet: async () => null,
    runCommand: async () => ({ accepted: true, operation_id: null }),
  });
});

beforeEach(() => {
  vi.clearAllMocks();
  native.statusCalls = 0;
  resetSeq();
  setTabs({
    tabs: [tab(TAB, "https://news.ycombinator.com/", "Hacker News")],
    active: TAB,
    space: SPACE,
  });
});

async function open(next: PageAssistStatusV1) {
  native.status = next;
  await page.viewport(400, 820);
  const onclose = vi.fn();
  const screen = await render(PanelStage, { profile: PROFILE, onclose });
  const field = screen.getByRole("textbox", { name: "Ask Zephium…" });
  await expect.element(field).toBeVisible();
  await expect.poll(() => native.statusCalls).toBeGreaterThan(0);
  return { screen, field, onclose };
}

test("Enter sends with the page in front; Shift+Enter only adds a line; × leaves the page out", async () => {
  const { screen, field } = await open(status());
  await expect.element(field).toHaveFocus();
  await field.fill("What's worth reading?");
  await userEvent.keyboard("{Shift>}{Enter}{/Shift}");
  expect(native.ask).not.toHaveBeenCalled();
  await userEvent.keyboard("{Enter}");
  await expect.poll(() => native.ask.mock.calls.length).toBe(1);
  expect(native.ask).toHaveBeenLastCalledWith({
    space: SPACE,
    conversation: CONVERSATION,
    text: "What's worth reading?",
    suggestion: null,
    attachments: [{ kind: "page", tab: TAB }],
  });
  await expect.element(field).toHaveValue("");

  await screen.getByRole("button", { name: "Remove news.ycombinator.com" }).click();
  await field.fill("Just between us");
  await userEvent.keyboard("{Enter}");
  await expect.poll(() => native.ask.mock.calls.length).toBe(2);
  expect(native.ask).toHaveBeenLastCalledWith(expect.objectContaining({ attachments: [] }));
});

test("a suggestion sends itself with the page", async () => {
  const { screen } = await open(status());
  await screen.getByRole("button", { name: "Worth opening" }).click();
  await expect.poll(() => native.ask.mock.calls.length).toBe(1);
  expect(native.ask).toHaveBeenLastCalledWith(
    expect.objectContaining({
      text: "",
      suggestion: "picks",
      attachments: [{ kind: "page", tab: TAB }],
    }),
  );
});

test("Escape closes the panel when chrome focus is outside its content", async () => {
  const onclose = vi.fn();
  native.status = status();
  const screen = await render(PanelStage, { profile: PROFILE, onclose });
  const outside = document.createElement("button");
  screen.container.append(outside);
  outside.focus();
  outside.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Escape", bubbles: true, isComposing: true }),
  );
  expect(onclose).not.toHaveBeenCalled();
  await userEvent.keyboard("{Escape}");
  expect(onclose).toHaveBeenCalledOnce();
});

test("the header offers a visible close button even while a reply is working", async () => {
  const onclose = vi.fn();
  native.status = status([person(1, "Summarize"), reply(2, [], { state: "working", cost: null })]);
  const screen = await render(PanelStage, { profile: PROFILE, onclose });
  const close = screen.getByRole("button", { name: "Close tool", exact: true });
  await expect.element(close).toBeVisible();
  await close.click();
  expect(onclose).toHaveBeenCalledOnce();
});

test("while a reply works, Send is Stop, and Escape stops it before closing anything", async () => {
  const onclose = vi.fn();
  native.status = status([person(1, "Summarize"), reply(2, [], { state: "working", cost: null })]);
  await page.viewport(400, 820);
  const screen = await render(PanelStage, { profile: PROFILE, onclose });
  const field = screen.getByRole("textbox", { name: "Ask Zephium…" });
  await expect.element(screen.getByRole("button", { name: "Stop" })).toBeVisible();
  await field.click();
  await userEvent.keyboard("{Escape}");
  await expect.poll(() => native.stop.mock.calls.length).toBe(1);
  expect(native.stop).toHaveBeenCalledWith(SPACE);
  expect(onclose).not.toHaveBeenCalled();
  await screen.getByRole("button", { name: "Stop" }).click();
  await expect.poll(() => native.stop.mock.calls.length).toBe(2);

  // Rust's snapshot moves on with its events; a focus reads it again.
  native.status = status([person(1, "Summarize"), reply(2, [text("Done.")])]);
  native.status.conversation.seq = 42;
  emitNativeEvent("pageAssistChanged", event(2, { type: "text", block: 0, delta: "Done." }));
  emitNativeEvent("pageAssistChanged", event(2, { type: "done", cost: null }));
  await expect.element(screen.getByRole("button", { name: "Dictate" })).toBeVisible();
  await field.click();
  await userEvent.keyboard("{Escape}");
  expect(onclose).toHaveBeenCalledOnce();
});

test("one round button: the mic on an empty field, Send once there are words", async () => {
  const { screen, field } = await open(status());
  await expect.element(screen.getByRole("button", { name: "Dictate" })).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Send" })).toHaveAttribute("inert");
  await field.fill("Hi");
  await expect.element(screen.getByRole("button", { name: "Send" })).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Dictate" })).toHaveAttribute("inert");
  await field.fill("");
  await expect.element(screen.getByRole("button", { name: "Dictate" })).toBeVisible();
});

test("the starts are a list the arrows walk, from the field up and back down", async () => {
  const { screen, field } = await open(status());
  await expect.element(screen.getByText("What can I do on Hacker News?")).toBeVisible();
  await field.click();
  await userEvent.keyboard("{ArrowUp}");
  await expect.element(screen.getByRole("button", { name: "Redesign this page" })).toHaveFocus();
  await userEvent.keyboard("{ArrowUp}");
  await expect.element(screen.getByRole("button", { name: "Key points" })).toHaveFocus();
  await userEvent.keyboard("{ArrowDown}{ArrowDown}");
  await expect.element(field).toHaveFocus();
});

test("+ attaches another tab of the Space alongside the page in front", async () => {
  setTabs({
    tabs: [
      tab(TAB, "https://news.ycombinator.com/", "Hacker News"),
      tab("tab-2", "https://github.com/zephium", "Zephium · GitHub"),
      tab("tab-3", "about:blank", ""),
    ],
  });
  const { screen, field } = await open(status());
  await screen.getByRole("button", { name: "Add" }).click();
  await page.getByRole("menuitem", { name: "Attach another tab" }).click();
  await expect
    .element(page.getByRole("menuitem", { name: /about:blank/u }))
    .not.toBeInTheDocument();
  await page.getByRole("menuitem", { name: /Zephium · GitHub/u }).click();
  await expect.element(screen.getByRole("button", { name: "Remove github.com" })).toBeVisible();
  await field.fill("Compare these");
  await userEvent.keyboard("{Enter}");
  await expect.poll(() => native.ask.mock.calls.length).toBe(1);
  expect(native.ask).toHaveBeenLastCalledWith(
    expect.objectContaining({
      attachments: [
        { kind: "page", tab: TAB },
        { kind: "page", tab: "tab-2" },
      ],
    }),
  );
  await expect
    .element(screen.getByRole("button", { name: "Remove github.com" }))
    .not.toBeInTheDocument();
});

test("an item's card points at its mark while hovered; ⌘-click opens it in a new tab", async () => {
  const { screen } = await open(status([person(1, "Picks?"), reply(2, [text("Three."), ITEMS])]));
  const first = screen.getByRole("button", { name: /eMudhra to exit/u });
  await first.hover();
  await expect.poll(() => native.focus.mock.calls.at(-1)).toEqual([CONVERSATION, 2, 1, 1]);
  await screen.getByRole("textbox").hover();
  await expect.poll(() => native.focus.mock.calls.at(-1)).toEqual([CONVERSATION, 2, 1, null]);
  await first.click({ modifiers: ["Meta"] });
  await expect.poll(() => native.url.mock.calls.at(-1)).toEqual(["https://emudhra.com/", true]);
});

test("a confirm is answered on its own block, with the exact control", async () => {
  const { screen } = await open(
    status([
      person(1, "Upvote it"),
      reply(
        2,
        [
          text("One step needs you."),
          {
            kind: "steps",
            steps: [
              {
                index: 0,
                label: "Click “upvote”",
                category: "save",
                state: "confirm",
                reason: null,
              },
            ],
          },
        ],
        { state: "waiting", cost: null },
      ),
    ]),
  );
  await screen.getByRole("button", { name: "upvote", exact: true }).click();
  await expect.poll(() => native.decide.mock.calls.at(-1)).toEqual([CONVERSATION, 2, 1, "approve"]);
});

test("Continue in Work starts it; once started, Open in Work goes to Work", async () => {
  const { screen } = await open(
    status([person(1, "Set up the status page"), reply(2, [text("Work fits."), HANDOFF()])]),
  );
  await screen.getByRole("button", { name: "Continue in Work" }).click();
  await expect.poll(() => native.continueWork.mock.calls.at(-1)).toEqual([CONVERSATION, 2, 1]);
  emitNativeEvent(
    "pageAssistChanged",
    event(2, { type: "block", block: 1, value: HANDOFF({ state: "started", work: "w1" }) }),
  );
  await screen.getByRole("button", { name: "Open in Work" }).click();
  await expect.poll(() => native.surface.mock.calls.at(-1)).toEqual(["work"]);
});

test("Not now declines the handoff on its block", async () => {
  const { screen } = await open(status([person(1, "Set it up"), reply(2, [HANDOFF()])]));
  await screen.getByRole("button", { name: "Not now" }).click();
  await expect.poll(() => native.decide.mock.calls.at(-1)).toEqual([CONVERSATION, 2, 0, "decline"]);
});

test("the picker attaches what was picked; × detaches it; Escape cancels a pick", async () => {
  native.pick.mockResolvedValueOnce({
    pick: { tab: TAB, pick: "7", label: "Story list", host: "news.ycombinator.com" },
    fault: null,
  });
  const { screen, field, onclose } = await open(status());
  const pickFromMenu = async () => {
    await screen.getByRole("button", { name: "Add" }).click();
    await page.getByRole("menuitem", { name: "Pick an element" }).click();
  };
  await pickFromMenu();
  await expect.poll(() => native.pick.mock.calls.at(-1)).toEqual([SPACE, TAB]);
  await expect.element(screen.getByText("Story list")).toBeVisible();
  await field.fill("Summarize this part");
  await userEvent.keyboard("{Enter}");
  await expect.poll(() => native.ask.mock.calls.length).toBe(1);
  expect(native.ask).toHaveBeenLastCalledWith(
    expect.objectContaining({
      attachments: [
        { kind: "page", tab: TAB },
        { kind: "element", tab: TAB, pick: "7" },
      ],
    }),
  );
  await expect.element(screen.getByText("Story list")).not.toBeInTheDocument();

  native.pick.mockResolvedValueOnce({
    pick: { tab: TAB, pick: "8", label: "Pricing table", host: "news.ycombinator.com" },
    fault: null,
  });
  await pickFromMenu();
  await screen.getByRole("button", { name: "Remove Pricing table" }).click();
  await expect.poll(() => native.detach.mock.calls.at(-1)).toEqual([TAB, "8"]);

  native.pick.mockImplementationOnce(() => new Promise(() => {}));
  await pickFromMenu();
  await expect
    .element(screen.getByRole("status").filter({ hasText: "Esc to cancel" }))
    .toBeVisible();
  const outside = document.createElement("button");
  screen.container.append(outside);
  outside.focus();
  await userEvent.keyboard("{Escape}");
  await expect.poll(() => native.pickCancel.mock.calls.at(-1)).toEqual([TAB]);
  expect(onclose).not.toHaveBeenCalled();
});

test("history lists past conversations, opens one and deletes one", async () => {
  const { screen } = await open(status([person(1, "Hi"), reply(2, [text("Hello.")])]));
  await screen.getByRole("button", { name: "More" }).click();
  await page.getByRole("menuitem", { name: "History" }).click();
  const row = screen.getByRole("button", { name: /^Pricing on the Pro plan/u });
  await expect.element(row).toBeVisible();
  await screen.getByRole("button", { name: "Delete “Pricing on the Pro plan”" }).click();
  await expect.poll(() => native.remove.mock.calls.at(-1)).toEqual([SPACE, "c0"]);
  await expect.element(row).not.toBeInTheDocument();

  native.history.mockResolvedValueOnce({
    version: 2,
    conversations: [
      { id: "c0", title: "Pricing on the Pro plan", space: SPACE, updated_ms: Date.now() },
    ],
  });
  await screen.getByRole("button", { name: "History" }).click();
  await screen.getByRole("button", { name: "More" }).click();
  await page.getByRole("menuitem", { name: "History" }).click();
  await screen.getByRole("button", { name: /^Pricing on the Pro plan/u }).click();
  await expect.poll(() => native.open.mock.calls.at(-1)).toEqual([SPACE, "c0"]);
  await expect.element(screen.getByRole("log")).toBeVisible();
});

test("a gap in the events reads the conversation again; closing the panel lets the page go", async () => {
  const { screen } = await open(
    status([person(1, "Hi"), reply(2, [], { state: "working", cost: null })]),
  );
  const before = native.statusCalls;
  emitNativeEvent("pageAssistChanged", { ...event(2, { type: "cancelled" }), seq: 99 });
  await expect.poll(() => native.statusCalls).toBe(before + 1);
  screen.unmount();
  expect(native.close).toHaveBeenCalledWith(SPACE);
});
