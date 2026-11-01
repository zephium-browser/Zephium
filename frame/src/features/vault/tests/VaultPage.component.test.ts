import { afterEach, expect, test, vi } from "vitest";
import { page, userEvent } from "vitest/browser";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import primitives from "../../../styles/tokens/primitive.css?raw";
import tokens from "../../../styles/tokens.css?raw";
import browserCSS from "../../../styles/browser.css?raw";
import type { VaultCall, VaultEntry, VaultResponse } from "$shared/ipc/bindings";
import { notesTestServer } from "$shared/testing/notes/server";
import briefUrl from "./fixtures/brief.pdf?url";
import VaultHost from "./VaultHost.svelte";

const native = vi.hoisted(() => ({
  vault: vi.fn(),
  root: vi.fn(),
  notes: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    vaultCall: native.vault,
    vaultRoot: native.root,
    noteCall: native.notes,
  });
});
const realFetch = window.fetch.bind(window);
const style = document.createElement("style");
style.textContent = primitives + tokens.replace("@theme static", ":root") + browserCSS;
document.head.append(style);
afterEach(() => {
  delete document.documentElement.dataset.theme;
  localStorage.clear();
});

const shot = (name: string) =>
  page.screenshot({ path: `../../../../../target/vault-qa/${name}.png` });

/** An in-memory vault shaped like a real coursework folder. */
function fakeVault(pdf: Uint8Array) {
  const files = new Map<string, Uint8Array | null>();
  const folder = (path: string) => files.set(path, null);
  const file = (path: string, text: string | Uint8Array) =>
    files.set(path, typeof text === "string" ? new TextEncoder().encode(text) : text);
  for (const path of [
    "CCA",
    "DBKF",
    "DBKF/Year 1",
    "DBKF/Year 2",
    "DBKF/Year 2/Sem 1",
    "DBKF/Year 2/Sem 2",
    "DBKF/Year 2/Sem 2/AM2",
    "DBKF/Year 2/Sem 2/BLaw",
    "DBKF/Year 2/Sem 2/ITF",
    "DBKF/Year 2/Sem 2/ITF/CA1",
    "DBKF/Year 2/Sem 2/ITF/Topic 1",
    "DBKF/Year 2/Sem 2/ITF/Topic 2",
    "Journal",
  ])
    folder(path);
  file("DBKF/Year 2/Sem 2/ITF/CA1/[Due 16 Nov 1700h] ITF CA1 Group Project.pdf", pdf);
  file(
    "DBKF/Year 2/Sem 2/ITF/CA1/ITF CA1 tips.md",
    "# ITF CA1 tips\n\n## Slides\n\nStart early.\n\n## Video\n\nKeep it short.\n",
  );
  file("DBKF/Year 2/Sem 2/ITF/CA1/Marks.xlsx", new Uint8Array([0x50, 0x4b]));
  file("DBKF/Year 2/Sem 2/ITF/Topic 1/Chapter 2 notes.md", "# Chapter 2 notes\n\nTrade finance.\n");
  file("Welcome.md", "# Welcome\n");
  const entry = (path: string): VaultEntry => {
    const name = path.slice(path.lastIndexOf("/") + 1);
    const data = files.get(path);
    const at = name.lastIndexOf(".");
    return {
      name,
      path,
      kind: data === null ? "folder" : "file",
      ext: data === null || at <= 0 ? "" : name.slice(at + 1).toLowerCase(),
      size: String(data?.length ?? 0),
      modified: "1793500000000",
    };
  };
  const children = (dir: string) =>
    [...files.keys()]
      .filter(
        (path) =>
          (dir ? path.startsWith(`${dir}/`) : true) &&
          !path.slice(dir ? dir.length + 1 : 0).includes("/"),
      )
      .map(entry)
      .sort((a, b) =>
        a.kind === b.kind
          ? a.name.localeCompare(b.name, undefined, { numeric: true })
          : a.kind === "folder"
            ? -1
            : 1,
      );
  const call = (call: VaultCall): VaultResponse => {
    switch (call.kind) {
      case "list":
        return { kind: "entries", entries: children(call.dir) };
      case "stat":
        return files.has(call.path)
          ? { kind: "entry", entry: entry(call.path) }
          : { kind: "error", error: "not_found" };
      case "create_file": {
        const path = call.dir ? `${call.dir}/${call.name}` : call.name;
        file(path, call.text);
        return { kind: "entry", entry: entry(path) };
      }
      case "create_folder": {
        const path = call.dir ? `${call.dir}/${call.name}` : call.name;
        folder(path);
        return { kind: "entry", entry: entry(path) };
      }
      case "rename":
      case "move": {
        const name =
          call.kind === "rename" ? call.name : call.path.slice(call.path.lastIndexOf("/") + 1);
        const dir =
          call.kind === "rename"
            ? call.path.slice(0, Math.max(0, call.path.lastIndexOf("/")))
            : call.dest;
        const to = dir ? `${dir}/${name}` : name;
        for (const [path, data] of [...files]) {
          if (path === call.path || path.startsWith(`${call.path}/`)) {
            files.delete(path);
            files.set(to + path.slice(call.path.length), data);
          }
        }
        return { kind: "entry", entry: entry(to) };
      }
      case "trash": {
        for (const path of [...files.keys()])
          if (path === call.path || path.startsWith(`${call.path}/`)) files.delete(path);
        return { kind: "entry", entry: { ...entry("Welcome.md"), path: `.trash/x` } };
      }
      case "search": {
        const words = call.query.toLowerCase().split(/\s+/u).filter(Boolean);
        const hits = [...files.keys()]
          .filter((path) => words.every((word) => path.toLowerCase().includes(word)))
          .filter((path) => !call.folders_only || files.get(path) === null)
          .slice(0, call.limit)
          .map(entry);
        return { kind: "entries", entries: hits };
      }
      case "read": {
        const data = files.get(call.path);
        if (!data) return { kind: "error", error: "not_found" };
        let binary = "";
        for (const byte of data) binary += String.fromCharCode(byte);
        return { kind: "bytes", base64: btoa(binary), modified: "1793500000000" };
      }
      case "downloads":
        return {
          kind: "entries",
          entries: [
            {
              name: "Lecture 3.pdf",
              path: "Lecture 3.pdf",
              kind: "file",
              ext: "pdf",
              size: "9",
              modified: "1",
            },
          ],
        };
      case "import": {
        const path = call.dest ? `${call.dest}/${call.source}` : call.source;
        file(path, pdf);
        return { kind: "entry", entry: entry(path) };
      }
      default:
        return { kind: "done" };
    }
  };
  return { files, call };
}

async function setup(theme: "dark" | "light" = "dark") {
  await page.viewport(1600, 960);
  document.documentElement.dataset.theme = theme;
  const pdf = new Uint8Array(await (await fetch(briefUrl)).arrayBuffer());
  const vault = fakeVault(pdf);
  native.vault.mockImplementation(async (_profile: string, call: VaultCall) => vault.call(call));
  native.root.mockResolvedValue("C:\\Users\\SP\\OneDrive\\Vault");
  const profile = "01J9ZQ3V6Q4M8Y2K7T5R1N0C77";
  const notes = notesTestServer(profile);
  native.notes.mockImplementation(notes.call);
  notes.seed("# ITF CA1 tips\n\n## Slides\n\nStart early.\n\n## Video\n\nKeep it short.\n", {
    path: "DBKF/Year 2/Sem 2/ITF/CA1/ITF CA1 tips.md",
  });
  // Viewers fetch file bytes from the vault scheme; serve them from the fake.
  vi.spyOn(window, "fetch").mockImplementation(async (input, init) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    if (!url.includes("zephium-vault")) return realFetch(input, init);
    const path = decodeURIComponent(url.split("/").at(-1) ?? "").replace(/^[^/]+\//u, "");
    const data = vault.files.get(path);
    return data ? new Response(new Blob([data as BlobPart])) : new Response(null, { status: 404 });
  });
  const screen = await render(VaultHost, { profile });
  await expect.element(screen.getByRole("treeitem", { name: "DBKF" })).toBeVisible();
  return { screen, vault };
}

test("the vault opens a coursework PDF beside its notes, Obsidian style", async () => {
  const { screen } = await setup();
  await shot("01-tree");

  for (const folder of ["DBKF", "Year 2", "Sem 2", "ITF", "CA1"]) {
    await screen.getByRole("treeitem", { name: folder, exact: true }).click();
  }
  await screen.getByRole("treeitem", { name: /Group Project/u }).click();
  await expect.element(screen.getByRole("tab", { name: /Group Project/u })).toBeVisible();
  await expect.element(screen.getByText("Grade Breakdown")).toBeVisible();
  await expect.element(screen.getByText("of 2")).toBeVisible();
  await shot("02-pdf");

  // Ctrl+click opens the notes file beside the PDF.
  await screen.getByRole("treeitem", { name: "ITF CA1 tips" }).click({ modifiers: ["Control"] });
  await expect.element(screen.getByRole("tab", { name: /ITF CA1 tips/u })).toBeVisible();
  await expect.element(screen.getByRole("button", { name: "Slides" })).toBeVisible();
  await expect.element(screen.getByRole("textbox", { name: "Note" })).toBeVisible();
  await shot("03-split");
});

test("keyboard: quick open, key tips, F2 rename and the four-pane grid", async () => {
  const { screen, vault } = await setup("light");
  await screen.getByRole("tree").click();
  await userEvent.keyboard("{Control>}o{/Control}");
  await userEvent.keyboard("chapter");
  await expect.element(screen.getByRole("option", { name: /Chapter 2 notes/u })).toBeVisible();
  await shot("04-quick-open");
  await userEvent.keyboard("{Enter}");
  await expect.element(screen.getByRole("tab", { name: /Chapter 2 notes/u })).toBeVisible();

  await screen.getByRole("radio", { name: "Four panes" }).click();
  await expect.element(screen.getByRole("region", { name: "Pane 4" })).toBeVisible();

  await userEvent.keyboard("{Alt}");
  await expect.element(screen.getByText("Q", { exact: true })).toBeVisible();
  await shot("05-key-tips");
  await userEvent.keyboard("q");
  await expect.element(screen.getByRole("region", { name: "Pane 2" })).not.toBeInTheDocument();

  await screen.getByRole("treeitem", { name: "Welcome" }).click();
  await screen.getByRole("tree").click();
  await userEvent.keyboard("{F2}");
  const field = screen.getByRole("textbox", { name: "Rename" });
  await expect.element(field).toHaveValue("Welcome");
  await userEvent.keyboard("{Control>}a{/Control}Start here{Enter}");
  await expect.poll(() => vault.files.has("Start here.md")).toBe(true);
});

test("a download is filed into a course folder with the keyboard", async () => {
  const { screen, vault } = await setup();
  await screen.getByRole("button", { name: "Downloads" }).click();
  await screen.getByRole("button", { name: /Lecture 3\.pdf/u }).click();
  await userEvent.keyboard("itf topic 1");
  await expect.element(screen.getByRole("option", { name: /Topic 1/u })).toBeVisible();
  await shot("06-file-download");
  await userEvent.keyboard("{Enter}");
  await expect
    .poll(() => vault.files.has("DBKF/Year 2/Sem 2/ITF/Topic 1/Lecture 3.pdf"))
    .toBe(true);
  await expect.element(screen.getByRole("tab", { name: /Lecture 3/u })).toBeVisible();
});

test("without a vault it asks for the folder", async () => {
  native.root.mockResolvedValue(null);
  native.vault.mockResolvedValue({ kind: "error", error: "no_root" });
  const screen = await render(VaultHost, { profile: "01J9ZQ3V6Q4M8Y2K7T5R1N0C78" });
  await expect.element(screen.getByRole("button", { name: "Choose vault folder" })).toBeVisible();
  await shot("00-welcome");
});
