import { afterEach, expect, test } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import { browserImport, type ImportAdapter, type ImportJob } from "$domain/browser-import";
import ImportBrowserData from "../components/ImportBrowserData.svelte";

afterEach(() => browserImport.provide(null));

function importer() {
  let emit: (job: ImportJob) => void = () => {};
  const started: unknown[] = [];
  const adapter: ImportAdapter = {
    sources: async () => [
      {
        id: "zen",
        browser: "zen",
        name: "Zen",
        profiles: [{ id: "p", name: "Default" }],
        kinds: ["essentials", "bookmarks", "history"],
        needsPermission: false,
        running: false,
      },
    ],
    start: async (...args) => {
      started.push(args);
      return true;
    },
    cancel: async () => {},
    onProgress: (listener) => {
      emit = listener;
      return () => {};
    },
    openPermissionSettings: async () => {},
  };
  return { adapter, started, emit: (job: ImportJob) => emit(job) };
}

const job = (finished: boolean, history: ImportJob["kinds"][number]): ImportJob => ({
  source: "zen",
  profile: "p",
  finished,
  cancelled: false,
  kinds: [
    { kind: "essentials", state: "done", done: 6, total: 6 },
    { kind: "bookmarks", state: "done", done: 0, total: 40 },
    history,
  ],
});

test("a refused start shows an error and a retry clears it", async () => {
  const fake = importer();
  let refuse!: (accepted: boolean) => void;
  const reply = new Promise<boolean>((resolve) => (refuse = resolve));
  let attempts = 0;
  fake.adapter.start = async () => (++attempts === 1 ? reply : true);
  browserImport.provide(fake.adapter);
  const screen = await render(ImportBrowserData);
  const start = screen.getByRole("button", { name: "Import", exact: true });

  await start.click();
  await expect.element(start).toHaveAttribute("aria-busy", "true");
  refuse(false);
  await expect
    .element(screen.getByRole("alert"))
    .toHaveTextContent("Couldn't start the import from Zen. Try again.");
  await start.click();
  expect(attempts).toBe(2);
  await expect.element(screen.getByRole("alert")).not.toBeInTheDocument();
});

test("a refused start names its source while an import from another browser progresses", async () => {
  const fake = importer();
  const list = fake.adapter.sources;
  fake.adapter.sources = async () => {
    const [zen] = await list();
    return [zen!, { ...zen!, id: "firefox", browser: "firefox", name: "Firefox" }];
  };
  let refuse!: (accepted: boolean) => void;
  const second = new Promise<boolean>((resolve) => (refuse = resolve));
  let attempts = 0;
  fake.adapter.start = async () => (++attempts === 1 ? true : second);
  browserImport.provide(fake.adapter);
  const screen = await render(ImportBrowserData);
  const start = screen.getByRole("button", { name: "Import", exact: true });

  await start.click();
  await screen.getByRole("radio", { name: "Firefox" }).click();
  await start.click();
  fake.emit(job(false, { kind: "history", state: "running", done: 0, total: null }));
  refuse(false);

  await expect.element(screen.getByText("Importing from Zen")).toBeVisible();
  await expect
    .element(screen.getByRole("alert"))
    .toHaveTextContent("Couldn't start the import from Firefox. Try again.");
});

test("an import shows each kind as it goes and what it brought", async () => {
  const fake = importer();
  browserImport.provide(fake.adapter);
  const screen = await render(ImportBrowserData);
  await expect.element(screen.getByRole("checkbox", { name: "Essentials" })).toBeChecked();
  await screen.getByRole("button", { name: "Import" }).click();
  expect(fake.started).toEqual([["zen", "p", ["essentials", "bookmarks", "history"]]]);

  fake.emit(job(false, { kind: "history", state: "running", done: 0, total: null }));
  await expect.element(screen.getByText("Importing from Zen")).toBeVisible();
  await expect.element(screen.getByText("6 added")).toBeVisible();
  await expect.element(screen.getByText("Already here")).toBeVisible();
  await expect.element(screen.getByText("Reading…")).toBeVisible();

  fake.emit(job(true, { kind: "history", state: "failed", done: 0, total: null, problem: "busy" }));
  await expect.element(screen.getByText("Imported from Zen")).toBeVisible();
  await expect.element(screen.getByText(/Quit Zen first/u)).toBeVisible();
  await expect
    .element(screen.getByRole("button", { name: "Allow access" }))
    .not.toBeInTheDocument();
  await screen.getByRole("button", { name: "Done" }).click();
  await expect.element(screen.getByRole("button", { name: "Import" })).toBeVisible();
});

test("a refusal by macOS says what to turn on and offers the setting", async () => {
  const fake = importer();
  const opened: string[] = [];
  fake.adapter.openPermissionSettings = async (source) => {
    opened.push(source);
  };
  browserImport.provide(fake.adapter);
  const screen = await render(ImportBrowserData);
  await screen.getByRole("button", { name: "Import" }).click();

  fake.emit({
    source: "zen",
    profile: "p",
    finished: true,
    cancelled: false,
    kinds: [{ kind: "bookmarks", state: "failed", done: 0, total: null, problem: "permission" }],
  });
  await expect.element(screen.getByText(/Full Disk Access to read Zen/u)).toBeVisible();
  await screen.getByRole("button", { name: "Allow access" }).click();
  expect(opened).toEqual(["zen"]);
});

test("a source with nothing to bring says so instead of claiming it is already here", async () => {
  const fake = importer();
  browserImport.provide(fake.adapter);
  const screen = await render(ImportBrowserData);
  await screen.getByRole("button", { name: "Import" }).click();

  fake.emit({
    source: "zen",
    profile: "p",
    finished: true,
    cancelled: false,
    kinds: [
      { kind: "bookmarks", state: "done", done: 0, total: 0 },
      { kind: "history", state: "done", done: 0, total: 40 },
    ],
  });
  await expect.element(screen.getByText("Nothing to import")).toBeVisible();
  await expect.element(screen.getByText("Already here")).toBeVisible();
});

test.each([0, 3])("an import stopped by a limit reports partial with %i added", async (done) => {
  const fake = importer();
  browserImport.provide(fake.adapter);
  const screen = await render(ImportBrowserData);
  await screen.getByRole("button", { name: "Import" }).click();
  fake.emit({
    source: "zen",
    profile: "p",
    finished: true,
    cancelled: false,
    kinds: [{ kind: "bookmarks", state: "partial", done, total: 40 }],
  });
  await expect
    .element(screen.getByText("Import stopped at a limit. Items already brought over stay."))
    .toBeVisible();
  await expect.element(screen.getByText("Already here")).not.toBeInTheDocument();
});
