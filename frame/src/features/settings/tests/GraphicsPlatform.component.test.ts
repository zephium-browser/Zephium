import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { handleNativeSection } from "../lib/settings-state.svelte";
import SettingsLookStage from "./SettingsLookStage.svelte";

const native = vi.hoisted(() => ({ asked: [] as string[] }));
vi.mock("$shared/platform", async (original) => ({
  ...(await original<typeof import("$shared/platform")>()),
  IS_LINUX: false,
  IS_MAC: true,
  IS_WINDOWS: false,
}));
vi.mock("$shared/ipc/bindings", () => ({
  commands: new Proxy(
    {},
    {
      get: (_target, key) =>
        typeof key === "string"
          ? async () => {
              native.asked.push(key);
              return null;
            }
          : undefined,
    },
  ),
}));

test("macOS and Windows have no Graphics page, and a link to it opens General", async () => {
  const screen = await render(SettingsLookStage);
  const navigation = screen.getByRole("navigation", { name: "Settings" });
  await expect.element(navigation.getByRole("button", { name: "About" })).toBeVisible();
  expect(navigation.getByRole("button", { name: "Graphics" }).query()).toBeNull();
  handleNativeSection("settings.section.graphics");
  await expect.element(screen.getByRole("heading", { level: 1, name: "General" })).toBeVisible();
  expect(native.asked).not.toContain("gpuReport");
});
