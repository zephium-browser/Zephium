import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import "$styles/global.css";
import type { BookmarkCall, BookmarkResponse, BookmarkView } from "$shared/ipc/bindings";
import * as compose from "../lib/compose.svelte";
import * as reveal from "../lib/reveal.svelte";
import BookmarksPanel from "../components/BookmarksPanel.svelte";

const view = (id: string, title: string, url: string | null, children = 0): BookmarkView => ({
  id,
  title,
  url,
  icon: null,
  children,
  parent: null,
});

const top = [
  view("1", "Reading", null, 2),
  view("2", "Zephium", "https://zephium.app/"),
  view("3", "Rust docs", "https://doc.rust-lang.org/std/"),
];
const reading = [view("4", "Long read", "https://essay.example/a")];

const native = vi.hoisted(() => ({
  call: vi.fn(),
  open: vi.fn(async () => ({ operation_id: null, accepted: true })),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ bookmarkCall: native.call, browserOpenUrl: native.open });
});

function answer(call: BookmarkCall): BookmarkResponse {
  if (call.kind === "list" && call.folder === "1") {
    return { kind: "listing", folder: "1", path: [{ id: "1", title: "Reading" }], items: reading };
  }
  if (call.kind === "reveal") return { kind: "listing", folder: null, path: [], items: top };
  if (call.kind === "list") return { kind: "listing", folder: null, path: [], items: top };
  return { kind: "saved", id: null };
}

afterEach(() => {
  vi.clearAllMocks();
  reveal.take();
});

test("browses folders, opens links, and renames in place", async () => {
  native.call.mockImplementation(async (_profile: string, call: BookmarkCall) => answer(call));
  const screen = await render(BookmarksPanel, { profile: "p", query: "" });
  await expect.element(screen.getByRole("button", { name: /Zephium/u })).toBeVisible();

  await screen.getByRole("button", { name: /Zephium/u }).click();
  expect(native.open).toHaveBeenLastCalledWith("https://zephium.app/", false);

  await screen
    .getByRole("button", { name: /Reading/u })
    .first()
    .click();
  await expect.element(screen.getByRole("button", { name: /Long read/u })).toBeVisible();
  await expect
    .element(screen.getByRole("button", { name: "Reading" }).last())
    .toHaveAttribute("aria-current", "location");

  await screen.getByRole("button", { name: "Rename" }).click();
  const field = screen.getByRole("textbox", { name: "Rename" });
  await userEvent.clear(field);
  await userEvent.type(field, "Essay{Enter}");
  await vi.waitFor(() =>
    expect(native.call).toHaveBeenCalledWith("p", { kind: "rename", id: "4", title: "Essay" }),
  );
});

test("a folder with contents asks in its row before it goes", async () => {
  native.call.mockImplementation(async (_profile: string, call: BookmarkCall) => answer(call));
  const screen = await render(BookmarksPanel, { profile: "p", query: "" });
  await screen.getByRole("button", { name: "Delete this folder and its 2 items" }).click();
  expect(native.call).not.toHaveBeenCalledWith("p", { kind: "remove", id: "1" });
  const question = screen.getByRole("group", { name: "Delete this folder and its 2 items" });
  await expect.element(question).toHaveTextContent(/Delete 2 Items/u);
  await question.getByRole("button", { name: "Cancel" }).click();
  await expect.element(question).not.toBeInTheDocument();

  await screen.getByRole("button", { name: "Delete this folder and its 2 items" }).click();
  await screen
    .getByRole("group", { name: "Delete this folder and its 2 items" })
    .getByRole("button", { name: "Delete 2 Items" })
    .click();
  await vi.waitFor(() =>
    expect(native.call).toHaveBeenCalledWith("p", { kind: "remove", id: "1" }),
  );
  // A folder cannot be put back, so it offers no undo.
  await expect.element(screen.getByRole("button", { name: "Undo" })).not.toBeInTheDocument();
});

test("a deleted link can be put back where it was", async () => {
  native.call.mockImplementation(async (_profile: string, call: BookmarkCall) =>
    call.kind === "add_link" ? { kind: "saved", id: "9" } : answer(call),
  );
  const screen = await render(BookmarksPanel, { profile: "p", query: "" });
  await screen.getByRole("button", { name: "Delete", exact: true }).nth(1).click();
  await vi.waitFor(() =>
    expect(native.call).toHaveBeenCalledWith("p", { kind: "remove", id: "3" }),
  );
  await expect.element(screen.getByRole("status")).toHaveTextContent("Deleted Rust docs");
  await screen.getByRole("button", { name: "Undo" }).click();
  await vi.waitFor(() =>
    expect(native.call).toHaveBeenCalledWith("p", {
      kind: "add_link",
      parent: null,
      title: "Rust docs",
      url: "https://doc.rust-lang.org/std/",
    }),
  );
  await vi.waitFor(() =>
    expect(native.call).toHaveBeenCalledWith("p", {
      kind: "move",
      id: "9",
      parent: null,
      index: 2,
    }),
  );
});

test("a requested bookmark opens where it lives, drawn apart", async () => {
  native.call.mockImplementation(async (_profile: string, call: BookmarkCall) => answer(call));
  reveal.request("2");
  const screen = await render(BookmarksPanel, { profile: "p", query: "" });
  await vi.waitFor(() =>
    expect(native.call).toHaveBeenCalledWith("p", { kind: "reveal", id: "2" }),
  );
  await expect.element(screen.getByRole("listitem").nth(1)).toHaveClass(/lit/u);
  document.documentElement.dataset.theme = "dark";
  await page.viewport(336, 360);
  await page.screenshot({ path: "../../../../../target/bookmarks-panel.png" });
  delete document.documentElement.dataset.theme;
});

test("adds a page by its address and says when it is not one", async () => {
  native.call.mockImplementation(async (_profile: string, call: BookmarkCall) => {
    if (call.kind === "add_link")
      return call.url === "not an address"
        ? { kind: "error", error: "invalid" }
        : { kind: "saved", id: "9" };
    return answer(call);
  });
  const screen = await render(BookmarksPanel, { profile: "p", query: "" });
  await expect.element(screen.getByRole("button", { name: /Zephium/u })).toBeVisible();
  // The header's Add menu asks; the panel opens the form.
  compose.request("link");
  const address = screen.getByRole("textbox", { name: "Address" });
  await userEvent.type(address, "not an address{Enter}");
  await expect.element(screen.getByRole("alert")).toHaveTextContent(/web address/u);

  await userEvent.clear(address);
  await userEvent.type(address, "example.com");
  await userEvent.type(screen.getByRole("textbox", { name: "Name (optional)" }), "Example");
  await screen.getByRole("button", { name: "Add", exact: true }).click();
  await vi.waitFor(() =>
    expect(native.call).toHaveBeenCalledWith("p", {
      kind: "add_link",
      parent: null,
      title: "Example",
      url: "example.com",
    }),
  );
  await expect.element(screen.getByRole("form")).not.toBeInTheDocument();
});

test("a new folder asked for from the header is made and named in place", async () => {
  native.call.mockImplementation(async (_profile: string, call: BookmarkCall) =>
    call.kind === "add_folder" ? { kind: "saved", id: "9" } : answer(call),
  );
  const screen = await render(BookmarksPanel, { profile: "p", query: "" });
  await expect.element(screen.getByRole("button", { name: /Zephium/u })).toBeVisible();
  compose.request("folder");
  await vi.waitFor(() =>
    expect(native.call).toHaveBeenCalledWith("p", expect.objectContaining({ kind: "add_folder" })),
  );
  expect(compose.requested()).toBeNull();
});

test("submits a bookmark only once while the add is pending, and allows retry after refusal", async () => {
  let finish!: (response: BookmarkResponse) => void;
  native.call.mockImplementation((_profile: string, call: BookmarkCall) =>
    call.kind === "add_link"
      ? new Promise<BookmarkResponse>((resolve) => (finish = resolve))
      : Promise.resolve(answer(call)),
  );
  const screen = await render(BookmarksPanel, { profile: "p", query: "" });
  await expect.element(screen.getByRole("button", { name: /Zephium/u })).toBeVisible();
  compose.request("link");
  const address = screen.getByRole("textbox", { name: "Address" });
  await userEvent.type(address, "example.com{Enter}{Enter}");
  const additions = () => native.call.mock.calls.filter(([, call]) => call.kind === "add_link");
  expect(additions()).toHaveLength(1);
  await expect.element(screen.getByRole("button", { name: "Add", exact: true })).toBeDisabled();
  await page.screenshot({ path: "../../../../../target/bookmark-add-pending.png" });

  finish({ kind: "error", error: "invalid" });
  await expect.element(screen.getByRole("alert")).toHaveTextContent(/web address/u);
  await expect.element(address).toHaveValue("example.com");
  await expect.element(screen.getByRole("button", { name: "Add", exact: true })).toBeEnabled();
  await userEvent.type(address, "{Enter}");
  expect(additions()).toHaveLength(2);
  finish({ kind: "saved", id: "9" });
  await expect.element(address).not.toBeInTheDocument();
});

test.each(["saved", "invalid"] as const)(
  "an old %s reply cannot close or mark a newly opened add form",
  async (result) => {
    let finish!: (response: BookmarkResponse) => void;
    native.call.mockImplementation((_profile: string, call: BookmarkCall) =>
      call.kind === "add_link"
        ? new Promise<BookmarkResponse>((resolve) => (finish = resolve))
        : Promise.resolve(answer(call)),
    );
    const screen = await render(BookmarksPanel, { profile: "p", query: "" });
    await expect.element(screen.getByRole("button", { name: /Zephium/u })).toBeVisible();
    compose.request("link");
    const address = screen.getByRole("textbox", { name: "Address" });
    await userEvent.type(address, "old.example{Enter}");
    await screen.getByRole("button", { name: "Cancel", exact: true }).click();
    compose.request("link");
    await userEvent.type(address, "new.example");
    finish(result === "saved" ? { kind: "saved", id: "9" } : { kind: "error", error: "invalid" });
    await new Promise(requestAnimationFrame);
    await expect.element(address).toHaveValue("new.example");
    await expect.element(screen.getByRole("alert")).not.toBeInTheDocument();
    await expect.element(screen.getByRole("button", { name: "Add", exact: true })).toBeEnabled();
  },
);

test("a pending add does not carry its form into another profile", async () => {
  let finish!: (response: BookmarkResponse) => void;
  native.call.mockImplementation((_profile: string, call: BookmarkCall) =>
    call.kind === "add_link"
      ? new Promise<BookmarkResponse>((resolve) => (finish = resolve))
      : Promise.resolve(answer(call)),
  );
  const screen = await render(BookmarksPanel, { profile: "p", query: "" });
  await expect.element(screen.getByRole("button", { name: /Zephium/u })).toBeVisible();
  compose.request("link");
  const address = screen.getByRole("textbox", { name: "Address" });
  await userEvent.type(address, "old.example{Enter}");
  await screen.rerender({ profile: "other", query: "" });
  await expect.element(address).not.toBeInTheDocument();
  compose.request("link");
  await userEvent.type(address, "new.example");
  finish({ kind: "saved", id: "9" });
  await new Promise(requestAnimationFrame);
  await expect.element(address).toHaveValue("new.example");
  expect(native.call.mock.calls.filter(([, call]) => call.kind === "add_link")).toEqual([
    ["p", { kind: "add_link", parent: null, title: "", url: "old.example" }],
  ]);
});

test("folders end in a chevron, not a count, and the top level has no crumb row", async () => {
  native.call.mockImplementation(async (_profile: string, call: BookmarkCall) => answer(call));
  const screen = await render(BookmarksPanel, { profile: "p", query: "" });
  await expect.element(screen.getByRole("button", { name: /Zephium/u })).toBeVisible();
  expect(screen.container.querySelector(".crumbs")).toBeNull();
  expect(screen.container.textContent).not.toMatch(/\d+ items/u);
  expect(screen.container.querySelectorAll(".chevron")).toHaveLength(1);
});

test("dragging a bookmark reorders it, and onto a folder files it there", async () => {
  native.call.mockImplementation(async (_profile: string, call: BookmarkCall) => answer(call));
  const screen = await render(BookmarksPanel, { profile: "p", query: "" });
  await expect.element(screen.getByRole("button", { name: /Rust docs/u })).toBeVisible();
  const row = (name: RegExp) => screen.getByRole("button", { name }).element() as HTMLElement;

  const carry = async (from: HTMLElement, to: HTMLElement, share: number) => {
    const start = from.getBoundingClientRect();
    const end = to.getBoundingClientRect();
    const at = { x: end.left + 20, y: end.top + end.height * share };
    const pointer = { pointerId: 1, bubbles: true, button: 0 };
    from.dispatchEvent(
      new PointerEvent("pointerdown", {
        ...pointer,
        clientX: start.left + 20,
        clientY: start.top + 5,
      }),
    );
    from.dispatchEvent(
      new PointerEvent("pointermove", { ...pointer, clientX: at.x, clientY: at.y }),
    );
    await new Promise(requestAnimationFrame);
    from.dispatchEvent(new PointerEvent("pointerup", { ...pointer, clientX: at.x, clientY: at.y }));
  };

  await carry(row(/Rust docs/u), row(/Zephium/u), 0.1);
  await vi.waitFor(() =>
    expect(native.call).toHaveBeenCalledWith("p", {
      kind: "move",
      id: "3",
      parent: null,
      index: 1,
    }),
  );

  await carry(row(/Zephium/u), row(/Reading/u), 0.5);
  await vi.waitFor(() =>
    expect(native.call).toHaveBeenCalledWith("p", {
      kind: "move",
      id: "2",
      parent: "1",
      index: 0xffff_ffff,
    }),
  );
});
