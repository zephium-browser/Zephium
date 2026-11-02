import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import "$styles/global.css";
import { emitNativeEvent } from "$shared/testing/native-events";
import type { SearchResult } from "$shared/ipc/bindings";
import NewTabSearch from "../components/NewTabSearch.svelte";

const native = vi.hoisted(() => ({
  context: {
    window_id: "window",
    profile_id: "profile",
    space_id: "space",
    session_id: "newtab:tab:1",
    request_id: "",
  },
  search: vi.fn(
    async (_query: string, _context: import("$shared/ipc/bindings").SearchContext) => true,
  ),
  run: vi.fn(async () => ({ accepted: true, operation_id: null })),
  cancel: vi.fn(async () => true),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    newtabSearchContext: async () => native.context,
    newtabSearch: native.search,
    newtabRun: native.run,
    newtabCancel: native.cancel,
  });
});
afterEach(() => {
  vi.clearAllMocks();
  delete document.documentElement.dataset.theme;
});

/** The field hangs in a notch whose height it fills, as on the page. */
async function mount(props: { tabId: string }) {
  const screen = await render(NewTabSearch, props);
  screen.container.style.blockSize = "50px";
  return screen;
}

const search = (title: string): SearchResult => ({
  kind: "search",
  title,
  detail: "DuckDuckGo",
  icon: null,
  action: { type: "OpenUrl", url: `https://duckduckgo.com/?q=${title}` },
});
const tab = (title: string, id = title): SearchResult => ({
  kind: "tab",
  title,
  detail: "rust-lang.org",
  icon: null,
  action: { type: "ActivateTab", id },
});

async function reply(query: string, results: SearchResult[], completion: string | null = null) {
  await vi.waitFor(() => expect(native.search.mock.calls.at(-1)?.[0]).toBe(query), {
    timeout: 3000,
  });
  const context = native.search.mock.calls.at(-1)![1];
  emitNativeEvent("searchChanged", { context, query, results, completion, pending: false });
  return context;
}

/** Answers the next request the surface makes, rather than re-answering one it
 *  has already had. Re-answering a stale request is rejected by design, so a
 *  test that does it proves nothing about what happens when an answer lands. */
/** Snapshot of the requests already made, taken before the interaction under
 *  test. The request can be issued while a later assertion is still polling,
 *  so the baseline cannot be captured after the fact. */
function issued() {
  return new Set(native.search.mock.calls.map((call) => call[1].request_id));
}

/** Answers the first request made after `seen`, rather than re-answering one
 *  the surface has already had. Re-answering a stale request is rejected by
 *  design, so a test that does it proves nothing about a landing answer. */
async function replyToNew(
  seen: Set<string>,
  query: string,
  results: SearchResult[],
  completion: string | null = null,
) {
  await vi.waitFor(
    () => expect(native.search.mock.calls.some((call) => !seen.has(call[1].request_id))).toBe(true),
    { timeout: 3000 },
  );
  return reply(query, results, completion);
}

test("the field is set into the notch and its results drop below it, best match first", async () => {
  const screen = await mount({ tabId: "tab" });
  const input = screen.getByRole("combobox");
  await expect.element(input).toHaveFocus();
  // The notch is the field's box, so the field draws none of its own, and it
  // speaks at the launcher's size rather than a form's.
  const field = screen.container.querySelector<HTMLElement>(".bar .ui-search")!;
  expect(getComputedStyle(field).backgroundColor).toBe("rgba(0, 0, 0, 0)");
  expect(getComputedStyle(field.querySelector("input")!).fontSize).toBe("15px");
  await userEvent.hover(field);
  expect(getComputedStyle(field).backgroundColor).toBe("rgba(0, 0, 0, 0)");
  expect(screen.container.querySelector(".sheet")).toBeNull();

  await input.fill("rust");
  await reply("rust", [
    search("rust"),
    { ...tab("Rust docs"), kind: "tab" },
    { ...search("Rust Book"), kind: "history", detail: "https://doc.rust-lang.org/book/" },
  ]);
  await expect.element(screen.getByRole("option", { name: /Rust docs/u })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Tabs", exact: true }).elements()).toHaveLength(0);

  const sheet = screen.container.querySelector<HTMLElement>(".sheet")!;
  expect(getComputedStyle(sheet).position).toBe("absolute");
  expect(sheet.getBoundingClientRect().top).toBeGreaterThanOrEqual(
    field.getBoundingClientRect().bottom,
  );
  // Read from the field downward: the typed search is the row against it.
  const options = [...screen.container.querySelectorAll('[role="option"]')];
  expect(options[0]!.textContent!.trim()).toMatch(/^rust/u);
  expect(options[0]!.getAttribute("aria-posinset")).toBe("1");

  await page.viewport(800, 600);
  screen.container.style.cssText =
    "width:620px;block-size:50px;padding:20px 20px 360px;background:var(--color-canvas)";
  document.documentElement.dataset.theme = "dark";
  await page
    .elementLocator(screen.container)
    .screenshot({ path: "../../../../../target/search-newtab-dark.png" });
  document.documentElement.dataset.theme = "light";
  await page
    .elementLocator(screen.container)
    .screenshot({ path: "../../../../../target/search-newtab-light.png" });
});

test("the list sizes itself to its content and does not scroll when it fits", async () => {
  const screen = await mount({ tabId: "tab" });
  await screen.getByRole("combobox").fill("rust");
  await reply("rust", [search("rust"), tab("Rust docs")]);
  const scroll = screen.container.querySelector<HTMLElement>(".scroll")!;
  await vi.waitFor(() => expect(scroll.clientHeight).toBeGreaterThan(0));
  // A height measured from the content box would omit the padding and leave a
  // scrollbar on a list that comfortably fits.
  expect(scroll.scrollHeight).toBeLessThanOrEqual(scroll.clientHeight);
});

test("Enter runs what was typed even when a local match exists", async () => {
  const screen = await mount({ tabId: "tab" });
  await screen.getByRole("combobox").fill("rust");
  // Native already orders the typed action first; the surface must not
  // re-point Enter at a local row just because one matched well.
  const context = await reply("rust", [search("rust"), tab("Rust docs")]);
  await userEvent.keyboard("{Enter}");
  await vi.waitFor(() =>
    expect(native.run).toHaveBeenCalledExactlyOnceWith(search("rust").action, context),
  );
});

test("Enter pressed before the answer runs what was typed once it lands", async () => {
  const screen = await mount({ tabId: "tab" });
  const input = screen.getByRole("combobox");
  await input.fill("rus");
  await reply("rus", [search("rus")]);
  // The list still shows the answer for "rus" when Enter is pressed.
  const seen = issued();
  await userEvent.type(input, "t");
  await userEvent.keyboard("{Enter}");
  expect(native.run).not.toHaveBeenCalled();
  const context = await replyToNew(seen, "rust", [search("rust")]);
  await vi.waitFor(() =>
    expect(native.run).toHaveBeenCalledExactlyOnceWith(search("rust").action, context),
  );
});

test("an Enter that commits a composition does not run the row", async () => {
  const screen = await mount({ tabId: "tab" });
  const field = screen.getByRole("combobox");
  const input = field.element() as HTMLInputElement;
  await field.fill("にほんご");
  await reply("にほんご", [search("にほんご")]);
  const seen = issued();
  // WebKit ends the composition before the keydown that committed it, which
  // then reports isComposing false and the IME's keyCode 229.
  input.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
  input.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: input.value }));
  const commit = new KeyboardEvent("keydown", {
    key: "Enter",
    keyCode: 229,
    bubbles: true,
    cancelable: true,
  });
  input.dispatchEvent(commit);
  expect(commit.defaultPrevented).toBe(false);
  const context = await replyToNew(seen, "にほんご", [search("にほんご")]);
  await new Promise((done) => setTimeout(done, 50));
  expect(native.run).not.toHaveBeenCalled();
  await userEvent.keyboard("{Enter}");
  await vi.waitFor(() =>
    expect(native.run).toHaveBeenCalledExactlyOnceWith(search("にほんご").action, context),
  );
});

test("selection moves without wrapping and returns to the typed action", async () => {
  const screen = await mount({ tabId: "tab" });
  await screen.getByRole("combobox").fill("rust");
  const context = await reply("rust", [search("rust"), tab("Rust docs"), tab("Rust book", "two")]);

  await userEvent.keyboard("{ArrowUp}");
  // Already at the top: nothing below may be selected by pressing up.
  await expect
    .element(screen.getByRole("option", { name: /^rust/u }))
    .toHaveAttribute("aria-selected", "true");

  await userEvent.keyboard("{ArrowDown}{ArrowDown}{ArrowDown}{ArrowDown}");
  await expect
    .element(screen.getByRole("option", { name: /Rust book/u }))
    .toHaveAttribute("aria-selected", "true");

  await userEvent.keyboard("{Enter}");
  await vi.waitFor(() =>
    expect(native.run).toHaveBeenCalledExactlyOnceWith(tab("Rust book", "two").action, context),
  );
});

test("a late-arriving result keeps the row the user aimed at", async () => {
  const screen = await mount({ tabId: "tab" });
  await screen.getByRole("combobox").fill("rust");
  const context = await reply("rust", [search("rust"), tab("Rust docs")]);
  await userEvent.keyboard("{ArrowDown}");
  emitNativeEvent("searchChanged", {
    context,
    query: "rust",
    completion: null,
    pending: false,
    results: [search("rust"), search("rust book"), tab("Rust docs")],
  });
  await expect
    .element(screen.getByRole("option", { name: /Rust docs/u }))
    .toHaveAttribute("aria-selected", "true");
});

test("a click placed before the answer arrives runs when it does", async () => {
  const screen = await mount({ tabId: "tab" });
  const input = screen.getByRole("combobox");
  await input.fill("rust");
  await reply("rust", [search("rust"), tab("Rust docs")]);

  // Replacement in flight: the visible rows no longer answer the question, so
  // they cannot execute yet. Doing nothing at all is what made clicks feel
  // broken, so the intent is held instead.
  await input.fill("rust d");
  await screen.getByRole("option", { name: /Rust docs/u }).click();
  expect(native.run).not.toHaveBeenCalled();

  const context = await reply("rust d", [search("rust d"), tab("Rust docs")]);
  await vi.waitFor(() =>
    expect(native.run).toHaveBeenCalledExactlyOnceWith(tab("Rust docs").action, context),
  );
});

test("Escape puts the list away, then clears the field", async () => {
  const screen = await mount({ tabId: "tab" });
  const input = screen.getByRole("combobox");
  await input.fill("rust");
  await reply("rust", [search("rust")]);
  expect(screen.container.querySelector(".sheet")).not.toBeNull();

  await userEvent.keyboard("{Escape}");
  // It fades on its way out, and is gone once it has.
  await expect.poll(() => screen.container.querySelector(".sheet")).toBeNull();
  await expect.element(input).toHaveValue("rust");

  await userEvent.keyboard("{Escape}");
  await expect.element(input).toHaveValue("");
});

test("the empty state waits for every provider before claiming nothing matched", async () => {
  const screen = await mount({ tabId: "tab" });
  await screen.getByRole("combobox").fill("zzz");
  await vi.waitFor(() => expect(native.search.mock.calls.at(-1)?.[0]).toBe("zzz"));
  const context = native.search.mock.calls.at(-1)![1];
  emitNativeEvent("searchChanged", {
    context,
    query: "zzz",
    completion: null,
    results: [],
    pending: true,
  });
  await expect.element(screen.getByText("No matching results")).not.toBeInTheDocument();
  emitNativeEvent("searchChanged", {
    context,
    query: "zzz",
    completion: null,
    results: [],
    pending: false,
  });
  await expect.element(screen.getByText("No matching results")).toBeInTheDocument();
});

test("Enter opens the completed host rather than searching the fragment typed", async () => {
  const screen = await mount({ tabId: "tab" });
  const input = screen.getByRole("combobox");
  await input.fill("not");
  const context = await reply(
    "not",
    [search("not"), { ...tab("Notion"), detail: "notion.so" }],
    "notion.so",
  );
  await vi.waitFor(() => expect((input.element() as HTMLInputElement).value).toBe("notion.so"));
  // The field reads "notion.so", so that is what the user is looking at and
  // what Enter must open. Searching "not" here is the wrong answer.
  await expect
    .element(screen.getByRole("option", { name: /Notion/u }))
    .toHaveAttribute("aria-selected", "true");
  await userEvent.keyboard("{Enter}");
  await vi.waitFor(() =>
    expect(native.run).toHaveBeenCalledExactlyOnceWith(
      { ...tab("Notion"), detail: "notion.so" }.action,
      context,
    ),
  );
});

test("backspace removes the completion instead of having it re-applied", async () => {
  const screen = await mount({ tabId: "tab" });
  const input = screen.getByRole("combobox");
  const element = input.element() as HTMLInputElement;
  await input.fill("you");
  await reply("you", [search("you"), { ...tab("YouTube"), detail: "youtube.com" }], "youtube.com");
  await vi.waitFor(() => expect(element.value).toBe("youtube.com"));
  // The appended part must still be selected, or Backspace is deleting a
  // character rather than rejecting the offer.
  expect([element.selectionStart, element.selectionEnd]).toEqual([3, 11]);

  // Deleting the completion leaves the typed text the same length, so judging
  // deletion against the typed text saw no change and immediately re-applied
  // the completion. Backspace then did nothing, however many times it was hit.
  let seen = issued();
  await userEvent.keyboard("{Backspace}");
  await vi.waitFor(() => expect(element.value).toBe("you"));
  await replyToNew(
    seen,
    "you",
    [search("you"), { ...tab("YouTube"), detail: "youtube.com" }],
    "youtube.com",
  );
  await expect.element(input).toHaveValue("you");

  seen = issued();
  await userEvent.keyboard("{Backspace}");
  await vi.waitFor(() => expect(element.value).toBe("yo"));
  await replyToNew(
    seen,
    "yo",
    [search("yo"), { ...tab("YouTube"), detail: "youtube.com" }],
    "youtube.com",
  );
  await expect.element(input).toHaveValue("yo");
});

test("an offered host completes the field and the appended part stays selected", async () => {
  const screen = await mount({ tabId: "tab" });
  const input = screen.getByRole("combobox");
  await input.fill("git");
  await reply("git", [search("git"), tab("GitHub")], "github.com");
  const element = input.element() as HTMLInputElement;
  await vi.waitFor(() => expect(element.value).toBe("github.com"));
  // The next keystroke must replace the offer, never append to it.
  expect(element.selectionStart).toBe(3);
  expect(element.selectionEnd).toBe(10);

  // Deleting is the user rejecting the offer; it must not be re-applied.
  await userEvent.keyboard("{Backspace}");
  await reply("git", [search("git"), tab("GitHub")], "github.com");
  await expect.element(input).toHaveValue("git");
});
