import { afterEach, expect, test } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import "$styles/global.css";
import { browserImport, type ImportAdapter, type ImportJob } from "$domain/browser-import";
import Import from "../components/steps/Import.svelte";

afterEach(() => browserImport.provide(null));

test("new import browsers render their own loaded artwork in both themes", async () => {
  const browsers = [
    ["helium", "Helium"],
    ["dia", "Dia"],
    ["comet", "Comet"],
    ["chromium", "Chromium"],
    ["floorp", "Floorp"],
    ["librewolf", "LibreWolf"],
    ["waterfox", "Waterfox"],
    ["opera-gx", "Opera GX"],
  ] as const;
  const fake = importer();
  fake.adapter.sources = async () =>
    browsers.map(([id, name]) => ({
      id,
      browser: id,
      name,
      profiles: [{ id: "Default", name: "Personal" }],
      kinds: ["bookmarks", "history"],
      needsPermission: false,
      running: false,
    }));
  browserImport.provide(fake.adapter);
  await browserImport.detect();
  const screen = await render(Import);
  await page.viewport(600, 760);
  try {
    for (const theme of ["light", "dark"]) {
      document.documentElement.dataset.theme = theme;
      for (const [, name] of browsers) {
        const choice = screen.getByRole("radio", { name, exact: true });
        await expect.element(choice).toBeVisible();
        await expect
          .poll(() => {
            const image = choice.element().querySelector("img");
            return image?.complete && image.naturalWidth > 0;
          })
          .toBe(true);
      }
      await Promise.all(
        document.getAnimations().map((animation) => animation.finished.catch(() => {})),
      );
      await page.screenshot({ path: `../../../../../target/onboarding/import-marks-${theme}.png` });
    }
  } finally {
    delete document.documentElement.dataset.theme;
  }
});

function importer() {
  let emit: (job: ImportJob) => void = () => {};
  const adapter: ImportAdapter = {
    sources: async () => [
      {
        id: "chrome",
        browser: "chrome",
        name: "Chrome",
        profiles: [{ id: "Default", name: "Person 1" }],
        kinds: ["bookmarks", "history"],
        needsPermission: false,
        running: false,
      },
    ],
    start: async () => true,
    cancel: async () => {},
    onProgress: (listener) => {
      emit = listener;
      return () => {};
    },
    openPermissionSettings: async () => {},
  };
  return { adapter, emit: (job: ImportJob) => emit(job) };
}

const finished = (kinds: ImportJob["kinds"]): ImportJob => ({
  source: "chrome",
  profile: "Default",
  finished: true,
  cancelled: false,
  kinds,
});

test("an import that found nothing says so instead of counting zero items brought over", async () => {
  const fake = importer();
  browserImport.provide(fake.adapter);
  await browserImport.detect();
  const screen = await render(Import);
  await screen.getByRole("radio", { name: "Chrome" }).click();
  await screen.getByRole("button", { name: "Import", exact: true }).click();

  fake.emit(
    finished([
      { kind: "bookmarks", state: "done", done: 0, total: 0 },
      { kind: "history", state: "done", done: 0, total: 0 },
    ]),
  );
  await expect.element(screen.getByText("Nothing to import")).toBeVisible();
  await expect.element(screen.getByText(/items brought over/u)).not.toBeInTheDocument();

  fake.emit(
    finished([
      { kind: "bookmarks", state: "done", done: 12, total: 12 },
      { kind: "history", state: "done", done: 0, total: 0 },
    ]),
  );
  await expect.element(screen.getByText("12 items brought over")).toBeVisible();
});

test("a refusal by macOS says what to turn on and offers the setting", async () => {
  const fake = importer();
  const opened: string[] = [];
  fake.adapter.openPermissionSettings = async (source) => {
    opened.push(source);
  };
  browserImport.provide(fake.adapter);
  await browserImport.detect();
  const screen = await render(Import);
  await screen.getByRole("radio", { name: "Chrome" }).click();
  await screen.getByRole("button", { name: "Import", exact: true }).click();

  fake.emit(
    finished([{ kind: "bookmarks", state: "failed", done: 0, total: null, problem: "permission" }]),
  );
  await expect.element(screen.getByText(/Full Disk Access to read Chrome/u)).toBeVisible();
  await screen.getByRole("button", { name: "Allow access" }).click();
  expect(opened).toEqual(["chrome"]);
});

test("a locked browser is not asked about access", async () => {
  const fake = importer();
  browserImport.provide(fake.adapter);
  await browserImport.detect();
  const screen = await render(Import);
  await screen.getByRole("radio", { name: "Chrome" }).click();
  await screen.getByRole("button", { name: "Import", exact: true }).click();

  fake.emit(
    finished([{ kind: "bookmarks", state: "failed", done: 0, total: null, problem: "busy" }]),
  );
  await expect.element(screen.getByText(/Quit Chrome first/u)).toBeVisible();
  await expect
    .element(screen.getByRole("button", { name: "Allow access" }))
    .not.toBeInTheDocument();
});
