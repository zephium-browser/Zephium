import { expect, test, vi } from "vitest";
import { page } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import SearchBlock from "../components/blocks/SearchBlock.svelte";
import { AssistSession } from "../lib/assist-session.svelte";
import type { PageAssistDecisionV1 } from "$shared/ipc/bindings";

const decideCommand = vi.hoisted(() => vi.fn());
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({ pageAssistDecide: decideCommand });
});

test("the exact search and destination are shown before any approval", async () => {
  const query = "Confidential “client” budget <script>71329</script> euros";
  const ondecide = vi.fn().mockResolvedValue(true);
  render(SearchBlock, {
    destination: "search.example",
    query,
    state: "confirm",
    inert: false,
    ondecide,
  });
  await expect.element(page.getByRole("heading", { name: "Search search.example?" })).toBeVisible();
  await expect.element(page.getByText(query, { exact: true })).toBeVisible();
  expect(ondecide).not.toHaveBeenCalled();
  await page.getByRole("button", { name: "Search", exact: true }).click();
  expect(ondecide).toHaveBeenCalledExactlyOnceWith("approve");
  await expect.element(page.getByRole("button", { name: "Search", exact: true })).toBeDisabled();
});

test("restored or stopped search proposals cannot be approved", async () => {
  const ondecide = vi.fn().mockResolvedValue(true);
  render(SearchBlock, {
    destination: "search.example",
    query: "private marker",
    state: "confirm",
    inert: true,
    ondecide,
  });
  await expect.element(page.getByText("private marker", { exact: true })).toBeVisible();
  await expect
    .element(page.getByRole("button", { name: "Search", exact: true }))
    .not.toBeInTheDocument();
  expect(ondecide).not.toHaveBeenCalled();
});

test("failed decision delivery restores controls and preserves the exact proposal", async () => {
  decideCommand
    .mockReset()
    .mockResolvedValueOnce(false)
    .mockRejectedValueOnce(new Error("offline"))
    .mockResolvedValue(true);
  const session = new AssistSession("profile");
  session.conversation = { id: "conversation", title: null, messages: [], seq: 0 };
  const ondecide = vi.fn((decision: PageAssistDecisionV1) => session.decide(2, 0, decision));
  render(SearchBlock, {
    destination: "https://search.example:8443",
    query: "private marker",
    state: "confirm",
    inert: false,
    ondecide,
  });
  const approve = page.getByRole("button", { name: "Search", exact: true });
  await approve.click();
  await expect.element(approve).toBeEnabled();
  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("Couldn’t send your choice. Try again.");
  await approve.click();
  await expect.element(approve).toBeEnabled();
  await expect.element(page.getByRole("alert")).toBeVisible();
  await expect.element(page.getByText("private marker", { exact: true })).toBeVisible();
  await approve.click();
  await expect.element(approve).toBeDisabled();
  await expect.element(page.getByRole("alert")).not.toBeInTheDocument();
  expect(ondecide).toHaveBeenCalledTimes(3);
});

test("an absent conversation cannot report decision delivery", async () => {
  decideCommand.mockClear();
  const session = new AssistSession("profile");
  expect(await session.decide(2, 0, "approve")).toBe(false);
  expect(decideCommand).not.toHaveBeenCalled();
});
