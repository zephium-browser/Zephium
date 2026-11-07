import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import type { PagePermissionPromptEntryView } from "$shared/ipc/bindings";
import SidebarPagePermission from "../components/SidebarPagePermission.svelte";

const native = vi.hoisted(() => ({
  prompt: null as PagePermissionPromptEntryView | null,
  release: vi.fn(),
  host: vi.fn(),
}));
vi.mock("$domain/permissions", () => ({
  pagePermissions: {
    prompt: () => native.prompt,
    busy: () => false,
    failure: () => null,
    respond: vi.fn(),
    hostInSidebar: () => {
      native.host();
      return native.release;
    },
  },
}));
afterEach(() => {
  vi.clearAllMocks();
  native.prompt = null;
});

const prompt = (in_sidebar: boolean): PagePermissionPromptEntryView => ({
  profile_id: "00000000000000000000000901",
  item_id: "00000000000000000000000902",
  request_id: "0000000000000007",
  origin: "https://meet.example",
  kinds: ["camera"],
  rememberable: true,
  processing: false,
  in_sidebar,
  blocked_by_system: false,
});

test("the sidebar holds prompts while it is mounted", async () => {
  const screen = await render(SidebarPagePermission);
  expect(native.host).toHaveBeenCalledTimes(1);
  await expect.element(screen.getByRole("alertdialog")).not.toBeInTheDocument();
  screen.unmount();
  expect(native.release).toHaveBeenCalledTimes(1);
});

test("only a prompt placed in the sidebar shows there", async () => {
  native.prompt = prompt(false);
  const sheet = await render(SidebarPagePermission);
  await expect.element(sheet.getByRole("alertdialog")).not.toBeInTheDocument();
  sheet.unmount();

  native.prompt = prompt(true);
  const card = await render(SidebarPagePermission);
  await expect.element(card.getByRole("alertdialog")).toBeVisible();
  await expect.element(card.getByText("meet.example wants to use your camera")).toBeVisible();
});
