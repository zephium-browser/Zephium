import "$styles/global.css";
import { beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import type { AccountFaultV1, AccountReplyV1, AccountV1 } from "$shared/ipc/bindings";
import { accountFixture, usageFixture } from "$shared/testing/account";
import AccountPage from "../components/sections/AccountPage.svelte";

const native = vi.hoisted(() => ({
  get: vi.fn(),
  pick: vi.fn(),
  save: vi.fn(),
  discard: vi.fn(),
  remove: vi.fn(),
}));
const PHOTO = `data:image/svg+xml,${encodeURIComponent(
  '<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8" fill="#c96"/></svg>',
)}`;
// Rust serves the picture by revision; here a picture stands in for it.
vi.mock("$domain/account/account-model", async (original) => ({
  ...(await original<typeof import("$domain/account/account-model")>()),
  avatarUrl: (revision: number | null | undefined) => (revision === 3 ? PHOTO : null),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    chromeModal: async () => {},
    accountGet: native.get,
    accountRefresh: async () => native.get(),
    accountAvatarPick: native.pick,
    accountAvatarSave: native.save,
    accountAvatarDiscard: native.discard,
    accountAvatarRemove: native.remove,
    profilesGet: async () => ({ status: "ok" as const, data: null }),
    settingGet: async () => null,
  });
});

const TOKEN = "01JABCDEFGHJKMNPQRSTVWXYZ0";
const reply = (
  account: AccountV1,
  fault: AccountFaultV1 | null = null,
  retry: number | null = null,
): AccountReplyV1 => ({ account, fault, retry_after: retry });

const plain = accountFixture({
  signed_in: true,
  email: "ada@example.com",
  name: "Ada Lovelace",
  plan: "free",
  usage: usageFixture(),
});
const pictured = accountFixture({ ...plain, avatar: 3 });

beforeEach(async () => {
  vi.clearAllMocks();
  await page.viewport(1100, 1400);
  native.get.mockResolvedValue(reply(plain));
  native.discard.mockImplementation(async () => reply(plain));
  native.pick.mockResolvedValue({
    status: "ok",
    data: { kind: "picked", token: TOKEN, width: 1600, height: 900 },
  });
});

test("the account's own picture is shown where its initials were", async () => {
  native.get.mockResolvedValue(reply(pictured));
  const screen = await render(AccountPage);
  const button = screen.getByRole("button", { name: "Change photo" });
  await expect.element(button).toBeVisible();
  expect(button.element().querySelector("img")?.getAttribute("src")).toBe(PHOTO);
  expect(button.element().textContent).not.toContain("AL");
});

test("choosing a photo frames it, and Save sends the framed square of the staged pick", async () => {
  native.save.mockResolvedValue(reply(pictured));
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Add a photo" }).click();
  const dialog = screen.getByRole("dialog", { name: "Position your photo" });
  await expect.element(dialog).toBeVisible();
  // The photo moved right with the keyboard, which frames more of its left, then zoomed a step.
  screen.getByRole("application").element().focus();
  await userEvent.keyboard("{Shift>}{ArrowRight}{ArrowRight}{ArrowRight}{ArrowRight}{/Shift}");
  await userEvent.keyboard("+");
  await screen.getByRole("button", { name: "Save" }).click();
  expect(native.save).toHaveBeenCalledTimes(1);
  const [token, crop] = native.save.mock.calls[0]!;
  expect(token).toBe(TOKEN);
  expect(crop.side).toBeLessThan(900);
  expect(crop.side).toBeGreaterThan(700);
  expect(crop.x).toBeLessThan(350);
  expect(crop.x + crop.side).toBeLessThanOrEqual(1600);
  expect(crop.y + crop.side).toBeLessThanOrEqual(900);
  await expect.element(dialog).not.toBeInTheDocument();
  expect(native.discard).not.toHaveBeenCalled();
  await expect.element(screen.getByRole("button", { name: "Change photo" })).toBeVisible();
});

test("cancelling puts the staged pick away; a refusal is said calmly and nothing else", async () => {
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Add a photo" }).click();
  await screen.getByRole("button", { name: "Cancel" }).click();
  await expect.element(screen.getByRole("dialog")).not.toBeInTheDocument();
  expect(native.discard).toHaveBeenCalledWith(TOKEN);

  native.pick.mockResolvedValue({ status: "ok", data: { kind: "unsupported" } });
  await screen.getByRole("button", { name: "Add a photo" }).click();
  await expect
    .element(screen.getByText("That picture can’t be used. Choose a still PNG, JPEG or WebP."))
    .toBeVisible();
  expect(screen.getByRole("dialog").elements()).toHaveLength(0);
});

test("a busy service is explained in the dialog with its wait; a cancelled upload says nothing", async () => {
  native.save.mockResolvedValueOnce(reply(plain, "avatar_busy", 12));
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Add a photo" }).click();
  await screen.getByRole("button", { name: "Save" }).click();
  await expect
    .element(screen.getByText("Photos are busy right now. Try again in 12s", { exact: false }))
    .toBeVisible();
  native.save.mockResolvedValueOnce(reply(plain, "avatar_cancelled"));
  await screen.getByRole("button", { name: "Save" }).click();
  await expect.element(screen.getByRole("alert")).not.toBeInTheDocument();
});

test("a photo can be replaced or removed from the picture's menu", async () => {
  native.get.mockResolvedValue(reply(pictured));
  native.remove.mockResolvedValue(reply(plain));
  const screen = await render(AccountPage);
  await screen.getByRole("button", { name: "Change photo" }).click();
  await expect.element(screen.getByRole("menuitem", { name: "Choose a new photo…" })).toBeVisible();
  await screen.getByRole("menuitem", { name: "Remove photo" }).click();
  expect(native.remove).toHaveBeenCalledTimes(1);
  await expect.element(screen.getByRole("button", { name: "Add a photo" })).toBeVisible();
});
