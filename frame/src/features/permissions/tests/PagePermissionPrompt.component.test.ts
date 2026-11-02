import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { userEvent } from "vitest/browser";
import "$styles/global.css";
import type { PagePermissionPromptEntryView } from "$shared/ipc/bindings";
import PagePermissionPrompt from "../components/PagePermissionPrompt.svelte";

const native = vi.hoisted(() => ({ respond: vi.fn() }));
vi.mock("$domain/permissions", () => ({
  pagePermissions: { busy: () => false, failure: () => null, respond: native.respond },
}));
afterEach(() => vi.clearAllMocks());

// Clicks in the first half second after the prompt appears are ignored.
const settle = () => new Promise((resolve) => setTimeout(resolve, 550));

const prompt = (rememberable: boolean, in_sidebar = false): PagePermissionPromptEntryView => ({
  profile_id: "00000000000000000000000901",
  item_id: "00000000000000000000000902",
  request_id: "0000000000000007",
  origin: "https://meet.example",
  kinds: ["camera", "microphone"],
  rememberable,
  processing: false,
  in_sidebar,
  blocked_by_system: false,
});

test("a saved profile offers the three choices and sends each one", async () => {
  const shown = prompt(true);
  const screen = await render(PagePermissionPrompt, { prompt: shown });
  await expect
    .element(screen.getByText("meet.example wants to use your camera and microphone"))
    .toBeVisible();
  await expect
    .element(screen.getByText("Private windows don't save this choice."))
    .not.toBeInTheDocument();
  await expect.element(screen.getByRole("button", { name: "Never allow" })).toHaveFocus();
  await settle();
  for (const [name, decision] of [
    ["Allow while visiting the site", "always_allow"],
    ["Allow this time", "allow_once"],
    ["Never allow", "always_deny"],
  ] as const) {
    await screen.getByRole("button", { name }).click();
    expect(native.respond).toHaveBeenLastCalledWith(shown, decision);
  }
});

test("a private profile offers only one-time choices", async () => {
  const shown = prompt(false);
  const screen = await render(PagePermissionPrompt, { prompt: shown });
  await expect.element(screen.getByText("Private windows don't save this choice.")).toBeVisible();
  await expect
    .element(screen.getByRole("button", { name: "Allow while visiting the site" }))
    .not.toBeInTheDocument();
  await expect.element(screen.getByRole("button", { name: "Never allow" })).not.toBeInTheDocument();
  await settle();
  await screen.getByRole("button", { name: "Allow this time" }).click();
  expect(native.respond).toHaveBeenLastCalledWith(shown, "allow_once");
  await screen.getByRole("button", { name: "Don't allow" }).click();
  expect(native.respond).toHaveBeenLastCalledWith(shown, "deny_once");
});

test("Escape dismisses the request so the page may ask again", async () => {
  const shown = prompt(true);
  const screen = await render(PagePermissionPrompt, { prompt: shown });
  await expect.element(screen.getByRole("button", { name: "Never allow" })).toHaveFocus();
  await userEvent.keyboard("{Escape}");
  expect(native.respond).toHaveBeenLastCalledWith(shown, "dismiss");
});

test("a location request names the exact site, scheme and port when not secure", async () => {
  const shown = {
    ...prompt(true),
    origin: "http://maps.example:8080",
    kinds: ["geolocation" as const],
  };
  const screen = await render(PagePermissionPrompt, { prompt: shown });
  await expect
    .element(screen.getByText("http://maps.example:8080 wants to know your location"))
    .toBeVisible();
  await settle();
  await screen.getByRole("button", { name: "Allow while visiting the site" }).click();
  expect(native.respond).toHaveBeenLastCalledWith(shown, "always_allow");
});

test("a location request says where to turn location on when macOS refuses it", async () => {
  const off = "Turn on Location Services for Zephium in System Settings › Privacy & Security.";
  const location = { ...prompt(true, true), kinds: ["geolocation" as const] };
  const allowed = await render(PagePermissionPrompt, { prompt: location });
  await expect.element(allowed.getByText(off)).not.toBeInTheDocument();
  allowed.unmount();

  const refused = await render(PagePermissionPrompt, {
    prompt: { ...location, blocked_by_system: true },
  });
  await expect.element(refused.getByText(off)).toBeVisible();
  await expect.element(refused.getByRole("alertdialog")).toHaveAccessibleDescription(off);
});

test("a prompt in the sidebar is a card that leaves the page in view", async () => {
  const shown = prompt(true, true);
  const screen = await render(PagePermissionPrompt, { prompt: shown });
  const card = screen.getByRole("alertdialog");
  await expect.element(card).toBeVisible();
  expect(card.element().closest(".fixed")).toBeNull();
  expect(card.element().getAttribute("aria-modal")).toBeNull();
  await expect
    .element(screen.getByText("meet.example wants to use your camera and microphone"))
    .toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Never allow" })).toHaveFocus();

  // A click that lands as the card appears is not an answer.
  await screen.getByRole("button", { name: "Allow this time" }).click();
  expect(native.respond).not.toHaveBeenCalled();
  await settle();
  await screen.getByRole("button", { name: "Allow this time" }).click();
  expect(native.respond).toHaveBeenLastCalledWith(shown, "allow_once");

  await screen.getByRole("button", { name: "Never allow" }).element().focus();
  await userEvent.keyboard("{Escape}");
  expect(native.respond).toHaveBeenLastCalledWith(shown, "dismiss");
});

test("the card leaves keys and focus with whatever else has them", async () => {
  const field = document.createElement("input");
  document.body.append(field);
  field.focus();
  const shown = prompt(true, true);
  const screen = await render(PagePermissionPrompt, { prompt: shown });
  await expect.element(screen.getByRole("alertdialog")).toBeVisible();
  expect(document.activeElement).toBe(field);
  await userEvent.keyboard("{Escape}");
  expect(native.respond).not.toHaveBeenCalled();
  field.remove();
});
