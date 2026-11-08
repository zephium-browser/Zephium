import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import "$styles/global.css";
import type { SitePermissionsView } from "$shared/ipc/bindings";
import SiteSettings from "../components/SiteSettings.svelte";

const native = vi.hoisted(() => ({
  list: vi.fn<(origin?: string) => Promise<SitePermissionsView | null>>(),
  set: vi.fn(async () => true),
  switchProfile: (_id: string) => {},
}));
vi.mock("$domain/permissions", () => ({ sitePermissions: native }));
vi.mock("$domain/tabs", async () => {
  const { SvelteMap } = await import("svelte/reactivity");
  const active = new SvelteMap([["id", "personal"]]);
  native.switchProfile = (id) => active.set("id", id);
  return { tabs: { profile: () => ({ id: active.get("id") }) } };
});
afterEach(() => {
  vi.resetAllMocks();
  native.set.mockResolvedValue(true);
  native.switchProfile("personal");
});

const saved: SitePermissionsView = {
  private: false,
  complete: true,
  kinds: ["camera", "microphone", "geolocation", "popups"],
  entries: [
    { origin: "https://meet.example", kind: "camera", decision: "allow", scheme: null },
    { origin: "https://maps.example", kind: "geolocation", decision: "block", scheme: null },
    { origin: "https://mail.example", kind: "popups", decision: "allow", scheme: null },
  ],
};

test("saved decisions are grouped by permission and can be changed or removed", async () => {
  native.list.mockResolvedValue(saved);
  render(SiteSettings);
  await expect.element(page.getByRole("region", { name: "Camera" })).toBeVisible();
  await expect.element(page.getByRole("region", { name: "Location" })).toBeVisible();
  await expect.element(page.getByRole("region", { name: "Pop-ups & redirects" })).toBeVisible();
  await expect.element(page.getByRole("region", { name: "Microphone" })).not.toBeInTheDocument();

  await page.getByRole("button", { name: "Location · https://maps.example" }).click();
  await page.getByRole("option", { name: "Allow" }).click();
  expect(native.set).toHaveBeenLastCalledWith("https://maps.example", "geolocation", "allow", null);

  await page.getByRole("button", { name: "Remove https://meet.example" }).click();
  expect(native.set).toHaveBeenLastCalledWith("https://meet.example", "camera", null, null);
  expect(native.list).toHaveBeenCalledTimes(3);
});

test("an app link grant names its scheme, offers no Block and is removed by scheme", async () => {
  const link = (scheme: string) => ({
    origin: "https://meet.example",
    kind: "app_links" as const,
    decision: "allow" as const,
    scheme,
  });
  native.list.mockResolvedValue({
    ...saved,
    kinds: [...saved.kinds, "app_links"],
    entries: [link("zoommtg"), link("msteams")],
  });
  render(SiteSettings);
  await expect.element(page.getByRole("region", { name: "Links to apps" })).toBeVisible();
  await expect.element(page.getByText("zoommtg links", { exact: true })).toBeVisible();
  await expect.element(page.getByText("msteams links", { exact: true })).toBeVisible();

  await page
    .getByRole("button", { name: "Links to apps · https://meet.example · zoommtg links" })
    .click();
  await expect.element(page.getByRole("option", { name: "Allow" })).toBeVisible();
  await expect.element(page.getByRole("option", { name: "Block" })).not.toBeInTheDocument();
  await page.getByRole("option", { name: "Allow" }).click();

  await page.getByRole("button", { name: "Remove https://meet.example · zoommtg links" }).click();
  expect(native.set).toHaveBeenLastCalledWith("https://meet.example", "app_links", null, "zoommtg");
});

test("a failed change says so", async () => {
  native.list.mockResolvedValue(saved);
  native.set.mockResolvedValueOnce(false);
  render(SiteSettings);
  await page.getByRole("button", { name: "Remove https://mail.example" }).click();
  await expect
    .element(page.getByRole("alert"))
    .toHaveTextContent("Couldn't change this site's permission.");
});

test("a profile switch during a read discards the old reply and reads the new profile", async () => {
  let finish!: (reply: SitePermissionsView) => void;
  const first = new Promise<SitePermissionsView>((resolve) => (finish = resolve));
  native.list.mockReturnValueOnce(first).mockResolvedValueOnce({ ...saved, entries: [] });
  await render(SiteSettings);
  await expect.poll(() => native.list.mock.calls.length).toBe(1);

  native.switchProfile("work");
  await expect.element(page.getByText("https://meet.example")).not.toBeInTheDocument();
  expect(native.list).toHaveBeenCalledTimes(1);
  finish(saved);

  await expect.element(page.getByText("Sites you allow or block will appear here.")).toBeVisible();
  await expect.element(page.getByText("https://meet.example")).not.toBeInTheDocument();
  expect(native.list).toHaveBeenCalledTimes(2);
});

test("switching profiles hides the old controls and ignores a late change failure", async () => {
  let finish!: (accepted: boolean) => void;
  const change = new Promise<boolean>((resolve) => (finish = resolve));
  native.list.mockResolvedValueOnce(saved).mockResolvedValueOnce({ ...saved, entries: [] });
  native.set.mockReturnValueOnce(change);
  await render(SiteSettings);

  const remove = page.getByRole("button", { name: "Remove https://mail.example" });
  await remove.click();
  native.switchProfile("work");
  await expect.element(remove).not.toBeInTheDocument();
  finish(false);

  await expect.element(page.getByText("Sites you allow or block will appear here.")).toBeVisible();
  await expect.element(page.getByRole("alert")).not.toBeInTheDocument();
  expect(native.set).toHaveBeenCalledTimes(1);
});

test("an empty, a private and an unreadable profile each say why there is no list", async () => {
  native.list.mockResolvedValueOnce({ ...saved, entries: [] });
  const empty = await render(SiteSettings);
  await expect.element(page.getByText("Sites you allow or block will appear here.")).toBeVisible();
  empty.unmount();

  native.list.mockResolvedValueOnce({ ...saved, private: true, entries: [] });
  const secret = await render(SiteSettings);
  await expect
    .element(page.getByText("Private windows don't keep site permissions."))
    .toBeVisible();
  secret.unmount();

  native.list.mockResolvedValueOnce({ ...saved, complete: false });
  const partial = await render(SiteSettings);
  await expect.element(page.getByText(/couldn't be read right now/)).toBeVisible();
  partial.unmount();

  native.list.mockResolvedValueOnce(null);
  render(SiteSettings);
  await expect.element(page.getByText("Site settings are unavailable right now.")).toBeVisible();
});
