import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import "$styles/global.css";
import type { SitePermissionsView } from "$shared/ipc/bindings";
import SiteSettings from "../components/SiteSettings.svelte";

const native = vi.hoisted(() => ({
  list: vi.fn<(origin?: string) => Promise<SitePermissionsView | null>>(),
  set: vi.fn(async () => true),
}));
vi.mock("$domain/permissions", () => ({ sitePermissions: native }));
afterEach(() => vi.clearAllMocks());

const saved: SitePermissionsView = {
  private: false,
  complete: true,
  kinds: ["camera", "microphone", "geolocation", "popups"],
  entries: [
    { origin: "https://meet.example", kind: "camera", decision: "allow" },
    { origin: "https://maps.example", kind: "geolocation", decision: "block" },
    { origin: "https://mail.example", kind: "popups", decision: "allow" },
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
  expect(native.set).toHaveBeenLastCalledWith("https://maps.example", "geolocation", "allow");

  await page.getByRole("button", { name: "Remove https://meet.example" }).click();
  expect(native.set).toHaveBeenLastCalledWith("https://meet.example", "camera", null);
  expect(native.list).toHaveBeenCalledTimes(3);
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
