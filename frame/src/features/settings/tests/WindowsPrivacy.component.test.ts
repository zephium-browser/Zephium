import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { emitNativeEvent } from "$shared/testing/native-events";
import WindowsPrivacy from "../components/WindowsPrivacy.svelte";

const native = vi.hoisted(() => ({
  diagnostics: vi.fn(),
  managed: vi.fn(),
  set: vi.fn(),
  value: "true",
}));
vi.mock("$domain/preferences", () => ({
  preferences: { value: () => native.value, saving: () => false, set: native.set },
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    windowsDiagnosticsSettings: native.diagnostics,
    windowsReputationManaged: native.managed,
  });
});
beforeEach(() => {
  native.value = "true";
  native.managed.mockResolvedValue(false);
});
afterEach(() => vi.clearAllMocks());

test("SmartScreen defaults on, explains disclosure and restart, and writes its own preference", async () => {
  const screen = await render(WindowsPrivacy);
  const toggle = page.getByRole("switch", { name: "Microsoft Defender SmartScreen" });
  await expect.element(toggle).toBeChecked();
  expect(screen.container.textContent).toContain("website addresses and download information");
  expect(screen.container.textContent).toContain("Restart Zephium to apply changes");
  expect(screen.container.textContent).toContain("required diagnostic data");
  expect(screen.container.textContent).toContain("even when optional diagnostics are off");
  await toggle.click();
  expect(native.set).toHaveBeenCalledWith("privacy.smartscreen", "false");
});

test("Windows diagnostic settings reports a failed launch and permits retry", async () => {
  native.diagnostics.mockResolvedValueOnce(false).mockResolvedValueOnce(true);
  await render(WindowsPrivacy);
  const button = page.getByRole("button", { name: "Windows diagnostic settings" });
  await button.click();
  await expect
    .element(page.getByRole("status"))
    .toHaveTextContent("Could not open Windows Settings");
  await button.click();
  await expect.element(page.getByRole("status")).not.toBeInTheDocument();
  expect(native.diagnostics).toHaveBeenCalledTimes(2);
});

test("failed native OFF application shows a managed switch without rewriting the preference", async () => {
  native.value = "false";
  native.managed.mockResolvedValue(true);
  await render(WindowsPrivacy);
  const toggle = page.getByRole("switch", { name: "Microsoft Defender SmartScreen" });
  await expect.element(toggle).toBeDisabled();
  await expect.element(page.getByText("Managed by Windows or your organization")).toBeVisible();
  expect(native.set).not.toHaveBeenCalled();
});

test("a later native refusal overrides a pending startup status query", async () => {
  let reply!: (managed: boolean) => void;
  native.managed.mockReturnValue(
    new Promise<boolean>((resolve) => {
      reply = resolve;
    }),
  );
  await render(WindowsPrivacy);
  emitNativeEvent("windowsReputationManaged", true);
  reply(false);
  await expect
    .element(page.getByRole("switch", { name: "Microsoft Defender SmartScreen" }))
    .toBeDisabled();
  await expect.element(page.getByText("Managed by Windows or your organization")).toBeVisible();
});

test("a saved OFF choice with unavailable launch read shows read-only ON and its local reason", async () => {
  native.value = "false";
  native.managed.mockResolvedValue(null);
  await render(WindowsPrivacy);
  const toggle = page.getByRole("switch", { name: "Microsoft Defender SmartScreen" });
  await expect.element(toggle).toBeChecked();
  await expect.element(toggle).toBeDisabled();
  await expect
    .element(
      page.getByText(
        "SmartScreen is on for this launch because your saved choice could not be read. Restart Zephium to try again.",
      ),
    )
    .toBeVisible();
  await expect
    .element(page.getByText("Managed by Windows or your organization"))
    .not.toBeInTheDocument();
  expect(native.set).not.toHaveBeenCalled();
});
