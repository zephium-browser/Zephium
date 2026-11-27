import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { blocker } from "$domain/blocker";
import ProtectionControls from "../components/ProtectionControls.svelte";

const copy = vi.hoisted(() => ({ localized: false }));
vi.mock("$shared/i18n/messages", async (importOriginal) => {
  const messages = await importOriginal<Record<string, unknown>>();
  return Object.fromEntries(
    Object.entries(messages).map(([key, message]) => [
      key,
      key.startsWith("settings_protection") && typeof message === "function"
        ? (...args: unknown[]) =>
            copy.localized ? `localized ${key}` : Reflect.apply(message, undefined, args)
        : message,
    ]),
  );
});

vi.mock("$domain/blocker", () => ({
  blocker: {
    status: vi.fn(() => ({
      protection: "disabled",
      preference: "authoritative",
      desired_enabled: false,
      can_enable: true,
      can_refresh_sources: true,
      source_phase: "fresh",
      retryable: false,
    })),
    refresh: vi.fn().mockResolvedValue(undefined),
    setEnabled: vi.fn(),
    refreshSources: vi.fn(),
  },
}));

afterEach(() => {
  vi.clearAllMocks();
  copy.localized = false;
});

test("the real toggle dispatches intent and never invents applied protection", async () => {
  let complete!: (result: blocker.BlockerMutationResult) => void;
  vi.mocked(blocker.setEnabled).mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        complete = resolve;
      }),
  );
  const screen = await render(ProtectionControls);
  const toggle = screen.getByRole("switch", { name: "Block ads and trackers" });
  await toggle.click();
  expect(blocker.setEnabled).toHaveBeenCalledExactlyOnceWith(true);
  await expect.element(toggle).toBeDisabled();
  expect(screen.container.textContent).toContain("Protection is off for this profile.");
  complete({ state: "unavailable" });
  await expect
    .element(screen.getByRole("status"))
    .toHaveTextContent("Could not confirm this change");
  await expect.element(toggle).toBeEnabled();
  expect(screen.container.textContent).not.toContain("Protection is active.");
});

test("filter refresh uses the real updater and reports pending settlement", async () => {
  vi.mocked(blocker.refreshSources).mockResolvedValueOnce({
    state: "pending",
    operation_id: "refresh",
  });
  const screen = await render(ProtectionControls);
  await screen.getByRole("button", { name: "Check for updates" }).click();
  expect(blocker.refreshSources).toHaveBeenCalledExactlyOnceWith();
  await expect.element(screen.getByRole("status")).toHaveTextContent("still being applied");
});

test.each([
  ["pending", "settings_protection_starting"],
  ["active", "settings_protection_active"],
  ["disabled", "settings_protection_off"],
  ["degraded", "settings_protection_attention"],
] as const)("protection controls use catalog text for %s", async (protection, key) => {
  copy.localized = true;
  vi.mocked(blocker.status).mockReturnValueOnce({
    ...blocker.status(),
    protection,
    retryable: true,
    desired_generation: "1",
  });
  const screen = await render(ProtectionControls);
  for (const message of [
    key,
    "settings_protection",
    "settings_protection_help",
    "settings_protection_enabled",
    "settings_protection_lists",
    "settings_protection_lists_help",
    "settings_protection_check_updates",
    "settings_protection_retry",
    "settings_protection_retry_help",
    "settings_protection_retry_action",
  ]) {
    expect(screen.container.textContent).toContain(`localized ${message}`);
  }
  await expect
    .element(screen.getByRole("switch", { name: "localized settings_protection_enabled" }))
    .toBeInTheDocument();
});

test.each(["pending", "unavailable"] as const)(
  "protection feedback uses catalog text for %s",
  async (state) => {
    copy.localized = true;
    vi.mocked(blocker.refreshSources).mockResolvedValueOnce(
      state === "pending" ? { state, operation_id: "refresh" } : { state },
    );
    const screen = await render(ProtectionControls);
    await screen
      .getByRole("button", { name: "localized settings_protection_check_updates" })
      .click();
    await expect
      .element(screen.getByRole("status"))
      .toHaveTextContent(
        state === "pending"
          ? "localized settings_protection_change_pending"
          : "localized settings_protection_change_failed",
      );
  },
);
