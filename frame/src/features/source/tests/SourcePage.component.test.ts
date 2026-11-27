import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import SourcePage from "../components/SourcePage.svelte";

const native = vi.hoisted(() => ({ source: vi.fn() }));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ pageSource: native.source as never });
});
vi.mock("$domain/surface", () => ({ surface: { close: vi.fn() } }));

afterEach(() => native.source.mockReset());

const document = (text: string, truncated = false) => ({
  url: "https://docs.example/guide",
  text,
  truncated,
});

test("returning to a tab does not admit its earlier source request", async () => {
  let finish!: (value: ReturnType<typeof document>) => void;
  native.source
    .mockReturnValueOnce(new Promise((resolve) => (finish = resolve)))
    .mockResolvedValueOnce(document("second tab"))
    .mockResolvedValueOnce(document("fresh source"));
  const screen = await render(SourcePage, { props: { tab: "first" } });
  await vi.waitFor(() => expect(native.source).toHaveBeenCalledTimes(1));
  await screen.rerender({ tab: "second" });
  await expect.element(page.getByText("second tab", { exact: true })).toBeVisible();
  await screen.rerender({ tab: "first" });
  await expect.element(page.getByText("fresh source", { exact: true })).toBeVisible();
  finish(document("stale source"));
  await new Promise(requestAnimationFrame);
  await expect.element(page.getByText("fresh source", { exact: true })).toBeVisible();
  await expect.element(page.getByText("stale source", { exact: true })).not.toBeInTheDocument();
});

test.each([false, true])(
  "a late copy does not confirm another source lifetime (return to first tab: %s)",
  async (returning) => {
    let finish!: () => void;
    const write = vi
      .fn()
      .mockReturnValueOnce(new Promise<void>((resolve) => (finish = resolve)))
      .mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText: write },
    });
    const first = document("first source");
    native.source
      .mockResolvedValueOnce(first)
      .mockResolvedValueOnce(document("second source"))
      .mockResolvedValueOnce(first);
    const screen = await render(SourcePage, { props: { tab: "first" } });
    await page.getByRole("button", { name: "Copy source" }).click();
    expect(write).toHaveBeenCalledExactlyOnceWith("first source");
    await screen.rerender({ tab: "second" });
    await expect.element(page.getByText("second source", { exact: true })).toBeVisible();
    if (returning) {
      await screen.rerender({ tab: "first" });
      await expect.element(page.getByText("first source", { exact: true })).toBeVisible();
    }
    finish();
    await new Promise(requestAnimationFrame);
    expect(screen.container.querySelector("header")?.textContent).toContain("Copy source");
    expect(screen.container.querySelector("header")?.textContent).not.toContain("Copied");
    await page.getByRole("button", { name: "Copy source" }).click();
    expect(write).toHaveBeenLastCalledWith(returning ? "first source" : "second source");
    await expect.element(page.getByRole("button", { name: "Copied", exact: true })).toBeVisible();
  },
);

test("the source is drawn as text, one numbered line each, never as markup", async () => {
  native.source.mockResolvedValue(
    document("<!doctype html>\n<script>alert(1)</script>\n\n<p>hi</p>"),
  );
  const screen = await render(SourcePage, { props: { tab: "tab-1" } });

  await expect.element(page.getByText("<!doctype html>")).toBeVisible();
  expect(native.source).toHaveBeenCalledExactlyOnceWith("tab-1");
  const lines = [...screen.container.querySelectorAll<HTMLElement>(".line")];
  expect(lines.map((line) => line.textContent)).toEqual([
    "<!doctype html>",
    "<script>alert(1)</script>",
    "",
    "<p>hi</p>",
  ]);
  expect(lines.map((line) => line.dataset.n)).toEqual(["1", "2", "3", "4"]);
  // Nothing in the source became an element.
  expect(screen.container.querySelector(".viewer script, .viewer p")).toBeNull();
  await expect.element(page.getByText("https://docs.example/guide")).toBeVisible();
  await expect.element(page.getByText("Lines: 4")).toBeVisible();
});

test("a source that is gone says so instead of showing an empty page", async () => {
  native.source.mockResolvedValue(null);
  await render(SourcePage, { props: { tab: "tab-2" } });

  await expect.element(page.getByRole("alert")).toHaveTextContent("no longer available");
  await expect.element(page.getByRole("button", { name: "Copy source" })).not.toBeInTheDocument();
});

test("a page too long to draw whole says only the first part is shown", async () => {
  native.source.mockResolvedValue(document("x\n".repeat(25_000)));
  const screen = await render(SourcePage, { props: { tab: "tab-3" } });

  await expect.element(page.getByRole("status")).toHaveTextContent("only the first part");
  expect(screen.container.querySelectorAll(".line").length).toBe(20_000);
  await expect.element(page.getByText("Lines: 25,001")).toBeVisible();
});

test("copy puts the whole source on the clipboard and confirms", async () => {
  const written: string[] = [];
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: async (text: string) => void written.push(text) },
  });
  native.source.mockResolvedValue(document("a\nb"));
  await render(SourcePage, { props: { tab: "tab-4" } });

  await page.getByRole("button", { name: "Copy source" }).click();

  expect(written).toEqual(["a\nb"]);
  await expect.element(page.getByRole("button", { name: "Copied" })).toBeVisible();
});
