import "$styles/global.css";
import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { preferences } from "$domain/preferences";
import AppearanceStage from "./AppearanceStage.svelte";

const native = vi.hoisted(() => ({ settings: {} as Record<string, string> }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    settingGet: async (key: string) => native.settings[key] ?? null,
    settingSet: async (key: string, value: string) => {
      native.settings[key] = value;
      return { accepted: true, operation_id: null };
    },
  });
});

const shots = "../../../../../target/look/appearance";
const settle = () => new Promise((done) => setTimeout(done, 450));

/** The page is taller than a window: shoot it in pages. */
async function shoot(name: string) {
  const scroller = document.querySelector<HTMLElement>(".settings-content")!;
  for (let part = 0; part * 1100 < scroller.scrollHeight; part++) {
    scroller.scrollTop = part * 1100;
    await new Promise((done) => setTimeout(done, 60));
    await page.screenshot({ path: `${shots}/${name}-${part + 1}.png` });
  }
  scroller.scrollTop = 0;
}

afterEach(() => {
  preferences.dispose();
  delete document.documentElement.dataset.material;
});

const cases: [string, "light" | "dark", string][] = [
  ["zephium", "dark", "liquid_glass"],
  ["zephium", "light", "liquid_glass"],
  ["paper", "light", "none"],
  ["paper", "dark", "none"],
  ["nord", "dark", "liquid_glass"],
  ["nord", "light", "none"],
  ["dracula", "dark", "none"],
  ["dracula", "light", "liquid_glass"],
  ["catppuccin", "dark", "liquid_glass"],
  ["catppuccin", "light", "none"],
];

for (const [id, mode, material] of cases) {
  test(`Appearance in ${id}, ${mode}, ${material}`, async () => {
    await page.viewport(1100, 1200);
    native.settings = { appearance: mode, "ui.theme": id, "ui.material": material };
    document.documentElement.dataset.theme = mode;
    document.documentElement.dataset.material = material;
    await preferences.init();
    await render(AppearanceStage);
    await settle();
    await shoot(`${id}-${mode}-${material}`);
  });
}

test("choosing the right sidebar edge survives reopening Appearance", async () => {
  native.settings = {};
  await preferences.init();
  const screen = await render(AppearanceStage);
  const position = screen.getByRole("button", { name: "Sidebar position", exact: true });
  await expect.element(position).toHaveTextContent("Left");
  await position.click();
  await page.getByRole("option", { name: "Right", exact: true }).click();
  await expect.poll(() => native.settings["sidebar.position"]).toBe("right");
  await expect.poll(() => preferences.saving()).toBe(false);
  screen.unmount();
  preferences.dispose();
  await preferences.init();
  const restored = await render(AppearanceStage);
  await expect
    .element(restored.getByRole("button", { name: "Sidebar position", exact: true }))
    .toHaveTextContent("Right");
});

test("editing a preset forks it into Custom and writes once it rests", async () => {
  await page.viewport(1100, 1200);
  native.settings = { appearance: "dark", "ui.theme": "nord" };
  document.documentElement.dataset.theme = "dark";
  await preferences.init();
  const screen = await render(AppearanceStage);
  const accent = screen.getByRole("textbox", { name: "Accent hex code" });
  await accent.fill("ff5500");
  await (accent.element() as HTMLInputElement).dispatchEvent(
    new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
  );
  await expect
    .poll(() => document.documentElement.style.getPropertyValue("--seed-dark-accent"))
    .toBe("#ff5500");
  expect(native.settings["ui.theme"]).toBe("nord");
  await expect.poll(() => native.settings["ui.theme"], { timeout: 3000 }).toBe("custom");
  expect(JSON.parse(native.settings["ui.theme-custom"]!)).toMatchObject({
    dark: { accent: "#ff5500" },
    from: "nord",
  });
  await expect.element(screen.getByText("Based on Nord").first()).toBeVisible();
  await settle();
  await page.screenshot({ path: `${shots}/custom-from-nord.png` });
  document.querySelector<HTMLElement>(".settings-content")!.scrollTop = 1000;
  await settle();
  await page.screenshot({ path: `${shots}/custom-from-nord-colors.png` });
  await screen.getByRole("button", { name: "Back to Nord" }).click();
  await expect.poll(() => native.settings["ui.theme"]).toBe("nord");
});

test("a copied theme imports back, and a broken one says so", async () => {
  await page.viewport(1100, 1200);
  native.settings = { appearance: "light", "ui.theme": "dracula" };
  document.documentElement.dataset.theme = "light";
  await preferences.init();
  const screen = await render(AppearanceStage);
  await screen.getByRole("button", { name: "Import" }).click();
  const field = screen.getByRole("textbox", { name: "Theme to import" });
  await field.fill("zephium-theme:v1:bm90IGEgdGhlbWU");
  await screen.getByRole("button", { name: "Use theme" }).click();
  await expect.element(screen.getByText("That isn’t a Zephium theme.")).toBeVisible();
  const scroller = document.querySelector<HTMLElement>(".settings-content")!;
  scroller.scrollTop = 900;
  await settle();
  await page.screenshot({ path: `${shots}/import-invalid-light.png` });

  const { palette } = await import("$domain/appearance");
  const shared = palette.encodeShare({
    spec: palette.resolveTheme("gruvbox", "").spec,
    contrast: 80,
  });
  await field.fill(shared);
  await screen.getByRole("button", { name: "Use theme" }).click();
  await expect.poll(() => native.settings["ui.theme"], { timeout: 3000 }).toBe("custom");
  expect(JSON.parse(native.settings["ui.theme-custom"]!)).toMatchObject({ from: "gruvbox" });
  await expect.poll(() => native.settings["ui.theme-contrast"]).toBe("80");
  scroller.scrollTop = 0;
  await settle();
  await shoot("imported-gruvbox-80-light");
});

test("contrast works on top of a preset and never forks it", async () => {
  await page.viewport(1100, 1200);
  native.settings = { appearance: "dark", "ui.theme": "nord", "ui.material": "liquid_glass" };
  document.documentElement.dataset.theme = "dark";
  document.documentElement.dataset.material = "liquid_glass";
  await preferences.init();
  const screen = await render(AppearanceStage);
  const slider = screen.getByRole("slider", { name: "Contrast" });
  (slider.element() as HTMLElement).focus();
  for (let step = 0; step < 3; step++)
    slider.element().dispatchEvent(new KeyboardEvent("keydown", { key: "PageUp", bubbles: true }));
  await expect
    .poll(() => document.documentElement.style.getPropertyValue("--seed-contrast"))
    .toBe("80");
  await expect.poll(() => native.settings["ui.theme-contrast"], { timeout: 3000 }).toBe("80");
  expect(native.settings["ui.theme"]).toBe("nord");
  expect(native.settings["ui.theme-custom"]).toBeUndefined();
  await expect.element(screen.getByText("Based on Nord")).not.toBeInTheDocument();
  const scroller = document.querySelector<HTMLElement>(".settings-content")!;
  scroller.scrollTop = 0;
  await settle();
  await shoot("nord-dark-contrast-80");
});

test("the mode pictures choose the mode by click and by arrow keys", async () => {
  await page.viewport(1100, 1200);
  native.settings = { appearance: "light", "ui.theme": "nord", "ui.material": "liquid_glass" };
  document.documentElement.dataset.theme = "light";
  document.documentElement.dataset.material = "liquid_glass";
  await preferences.init();
  const screen = await render(AppearanceStage);
  const modes = screen.getByRole("radiogroup", { name: "Appearance" });
  const dark = modes.getByRole("radio", { name: "Dark" });
  await dark.click();
  await expect.poll(() => native.settings.appearance).toBe("dark");
  await expect.element(dark).toBeChecked();
  const press = (key: string) =>
    (dark.element() as HTMLElement).dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true }),
    );
  press("ArrowRight");
  await expect.poll(() => native.settings.appearance).toBe("system");
  await expect.element(modes.getByRole("radio", { name: "System" })).toBeChecked();
  press("ArrowRight");
  await expect.poll(() => native.settings.appearance).toBe("light");
  await settle();
  await page.screenshot({ path: `${shots}/mode-nord-light-glass.png` });
});

test("the mode pictures fit a narrow window", async () => {
  await page.viewport(520, 900);
  native.settings = { appearance: "system", "ui.theme": "gruvbox", "ui.material": "none" };
  document.documentElement.dataset.theme = "dark";
  document.documentElement.dataset.material = "none";
  await preferences.init();
  await render(AppearanceStage);
  await settle();
  await page.screenshot({ path: `${shots}/mode-gruvbox-system-narrow.png` });
});
