import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import "$styles/global.css";
import { preferences } from "$domain/preferences";
import { emitNativeEvent } from "$shared/testing/native-events";
import type { SearchResult } from "$shared/ipc/bindings";
import type { HomeField } from "$domain/home";
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
  settled: { outcome: "applied", disposition: null } as unknown,
  ran: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    searchShortcutDefaults: async () => [],
    settingGet: async () => null,
    newtabSearchContext: async () => native.context,
    newtabSearch: native.search,
    newtabRun: native.run,
    newtabCancel: async () => true,
  });
});
vi.mock("$domain/operations", async (actual) => ({
  ...(await actual<object>()),
  settle: async (pending: Promise<unknown>) => {
    await pending;
    return native.settled;
  },
}));
afterEach(() => {
  preferences.dispose();
  native.settled = { outcome: "applied", disposition: null };
  vi.clearAllMocks();
});

const held: HomeField = {
  suggest: (query) =>
    query.startsWith("str")
      ? [
          { kind: "row", search: "q-1", entry: "pricing", label: "Pricing", place: "Stripe" },
          { kind: "search", search: "q-0", text: "stripe fees" },
        ]
      : [],
  run: (suggestion) => native.ran(suggestion),
};

async function mount(props: Record<string, unknown> = {}) {
  const screen = await render(NewTabSearch, {
    tabId: "tab",
    loadMenu: () => import("$shared/ui/Menu"),
    home: () => held,
    ...props,
  });
  screen.container.style.blockSize = "50px";
  return screen;
}

const search = (title: string): SearchResult => ({
  kind: "search",
  title,
  detail: "Google",
  icon: null,
  action: { type: "OpenUrl", url: `https://search.example/?q=${title}` },
});
const tab = (title: string): SearchResult => ({
  kind: "tab",
  title,
  detail: "stripe.com",
  icon: null,
  action: { type: "ActivateTab", id: "tab-9" },
});

async function reply(query: string, results: SearchResult[]) {
  await vi.waitFor(() => expect(native.search.mock.calls.at(-1)?.[0]).toBe(query), {
    timeout: 3000,
  });
  const context = native.search.mock.calls.at(-1)![1];
  emitNativeEvent("searchChanged", { context, query, results, completion: null, pending: false });
  return context;
}
const options = () =>
  [...document.querySelectorAll<HTMLElement>('[role="option"]')].map((row) =>
    row.textContent?.replace(/\s+/gu, " ").trim(),
  );

test("Home's rows stand after open tabs and before the search, which Enter still runs", async () => {
  const screen = await mount();
  await screen.getByRole("combobox").fill("stripe");
  const context = await reply("stripe", [search("stripe"), tab("Stripe dashboard")]);
  await expect.poll(() => options().length).toBe(4);
  const shown = options();
  expect(shown[0]).toContain("Stripe dashboard");
  expect(shown[1]).toContain("Pricing");
  expect(shown[2]).toContain("stripe fees");
  expect(shown[3]).toContain("stripe");
  await userEvent.keyboard("{Enter}");
  await vi.waitFor(() =>
    expect(native.run).toHaveBeenCalledExactlyOnceWith(search("stripe").action, context),
  );
  expect(native.ran).not.toHaveBeenCalled();
});

test("a Home row opens there by ids and the field is ready for the next search", async () => {
  const screen = await mount();
  const field = screen.getByRole("combobox");
  await field.fill("stripe");
  await reply("stripe", [search("stripe")]);
  await expect.poll(() => options().length).toBe(3);
  document.querySelectorAll<HTMLElement>('[role="option"]')[0]!.click();
  await vi.waitFor(() =>
    expect(native.ran).toHaveBeenCalledExactlyOnceWith(
      expect.objectContaining({ kind: "row", search: "q-1", entry: "pricing" }),
    ),
  );
  expect(native.run).not.toHaveBeenCalled();
  await expect.element(field).toHaveValue("");
  document.querySelector<HTMLInputElement>('[role="combobox"]')!.focus();
  await field.fill("stripe");
  await reply("stripe", [search("stripe")]);
  await expect.poll(() => options().length).toBe(3);
  document.querySelectorAll<HTMLElement>('[role="option"]')[1]!.click();
  await vi.waitFor(() =>
    expect(native.ran).toHaveBeenLastCalledWith({
      kind: "search",
      search: "q-0",
      text: "stripe fees",
    }),
  );
});

test("Command-Enter asks for a plain search, which Home never maps", async () => {
  const screen = await mount();
  await screen.getByRole("combobox").fill("stripe");
  const context = await reply("stripe", [search("stripe")]);
  await userEvent.keyboard("{Meta>}{Enter}{/Meta}");
  await vi.waitFor(() =>
    expect(native.run).toHaveBeenCalledExactlyOnceWith(search("stripe").action, {
      ...context,
      plain: true,
    }),
  );
});

test("without Home, Command-Enter is an ordinary search", async () => {
  const screen = await mount({ home: undefined });
  await screen.getByRole("combobox").fill("stripe");
  const context = await reply("stripe", [search("stripe")]);
  await userEvent.keyboard("{Meta>}{Enter}{/Meta}");
  await vi.waitFor(() =>
    expect(native.run).toHaveBeenCalledExactlyOnceWith(search("stripe").action, context),
  );
});

test("a search that became a map on Home clears the field and puts the list away", async () => {
  native.settled = { outcome: "applied", disposition: { reason: "home_map_started" } };
  const screen = await mount();
  const field = screen.getByRole("combobox");
  await field.fill("stripe");
  await reply("stripe", [search("stripe")]);
  await userEvent.keyboard("{Enter}");
  await expect.element(field).toHaveValue("");
  await expect.poll(() => options()).toEqual([]);
  expect(document.activeElement).toBe(field.element());
});

test("ArrowDown with no list open goes on to the page below; with a list it moves in it", async () => {
  const onleave = vi.fn();
  const screen = await mount({ onleave });
  const field = screen.getByRole("combobox");
  (field.element() as HTMLInputElement).focus();
  await userEvent.keyboard("{ArrowDown}");
  expect(onleave).toHaveBeenCalledTimes(1);
  await field.fill("stripe");
  await reply("stripe", [search("stripe")]);
  await expect.poll(() => options().length).toBe(3);
  await userEvent.keyboard("{ArrowDown}");
  expect(onleave).toHaveBeenCalledTimes(1);
});
