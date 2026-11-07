import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import * as tools from "$session/tools.svelte";
import type { WorkCallV1, WorkRuntimeProjection } from "$shared/ipc/bindings";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
import { documentMarkdown } from "../lib/work-notes";

const native = vi.hoisted(() => ({ work: vi.fn(), note: vi.fn(), resource: vi.fn() }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    workCall: native.work,
    noteCall: native.note,
    resourceCall: native.resource,
    sidebarSetHidden: async () => {},
    sidebarPeek: async () => {},
    sidebarSetWidth: async () => {},
  });
});

async function workspace(edit?: (state: WorkRuntimeProjection) => void) {
  await page.viewport(1200, 800);
  const { workSession } = await import("$domain/work");
  const { projection, snapshot } = await import("./environment-fixtures");
  const { notesTestServer } = await import("$shared/testing/notes/server");
  const { resourceTestServer } = await import("$shared/testing/resources/server");
  const notes = notesTestServer(snapshot.profile);
  native.note.mockImplementation(notes.call);
  native.resource.mockImplementation(resourceTestServer(snapshot.profile).call);
  const state = structuredClone(projection);
  edit?.(state);
  native.work.mockImplementation(async (_profile: string, call: WorkCallV1) => {
    if (call.kind === "query" && call.request.query.kind === "projection")
      return {
        version: 1,
        profile: snapshot.profile,
        reply: { kind: "projection", projection: structuredClone(state) },
      };
    if (call.kind === "environment" && call.request.kind === "checkpoint") {
      const request = call.request;
      return {
        version: 1,
        profile: snapshot.profile,
        reply: {
          kind: "environment",
          reply: {
            kind: "checkpointed",
            expected: request.expected,
            applied_view_revision: String(BigInt(request.expected) + 1n),
            replayed: false,
            snapshot: {
              ...environment.snapshot!,
              view: { ...request.view, revision: String(BigInt(request.expected) + 1n) },
            },
          },
        },
      };
    }
    return { version: 1, profile: snapshot.profile, reply: { kind: "error", error: "not_found" } };
  });
  const environment = new WorkEnvironmentSession(snapshot.profile, snapshot.space);
  environment.snapshot = structuredClone(snapshot);
  environment.selected = snapshot.id;
  const objective = workSession(snapshot.profile)!;
  vi.spyOn(objective, "start").mockResolvedValue();
  vi.spyOn(objective, "open").mockImplementation(async (id: string) => {
    objective.projection = structuredClone(state);
    objective.selected = id;
    return true;
  });
  const screen = await render(WorkEnvironmentWorkspace, {
    session: environment,
    tabs: [],
    spaceName: "Personal",
    aiEnabled: true,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "720px";
  root.style.width = "1100px";
  return {
    screen,
    notes,
    async close() {
      await screen.unmount();
      environment.dispose();
      objective.dispose();
    },
  };
}

test("a document result is saved as a note only when asked, and then opens it", async () => {
  const { screen, notes, close } = await workspace();
  const cover = () => screen.container.querySelector<HTMLElement>('[data-card-id="result-card"]');
  await expect.poll(cover, { timeout: 5000 }).not.toBeNull();
  // The line offers the same action beside the agent's follow-ups, for a document.
  const next = screen.getByRole("button", { name: "Next", exact: true });
  await expect.element(next).toBeVisible();
  expect(notes.calls.some((call) => call.kind === "create")).toBe(false);
  // A reviewed plan's result opens in the lift, where it is read and kept.
  cover()!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  cover()!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  const lift = screen.getByRole("dialog", { name: "Dependency findings" });
  await expect.element(lift).toBeVisible();
  await screen.getByRole("button", { name: "Save as note", exact: true }).click();
  await expect
    .element(screen.getByRole("button", { name: "Open note", exact: true }))
    .toBeVisible();
  const created = notes.calls.filter((call) => call.kind === "create");
  expect(created).toHaveLength(1);
  expect(created[0]).toMatchObject({
    kind: "create",
    markdown: "# Dependency findings\n\nReviewed findings\n",
  });
  // Written up, the line's offer is gone.
  await expect.element(next).not.toBeInTheDocument();
  await screen.getByRole("button", { name: "Open note", exact: true }).click();
  await expect.element(lift).not.toBeInTheDocument();
  // The note opens in the sidebar's Notes, beside the canvas.
  expect(tools.activeTool()).toBe("notes");
  tools.close();
  await close();
});

test("the line writes a document up from its row, and offers nothing for any other result", async () => {
  const first = await workspace();
  await first.screen.getByRole("button", { name: "Next", exact: true }).click();
  const row = first.screen.getByRole("button", { name: "Write this up as a note", exact: true });
  await expect.element(row).toBeVisible();
  await row.click();
  await expect
    .poll(() => first.notes.calls.filter((call) => call.kind === "create").length)
    .toBe(1);
  await expect
    .element(first.screen.getByRole("button", { name: "Next", exact: true }))
    .not.toBeInTheDocument();
  await first.close();

  const table = await workspace((state) => {
    const run = state.executions[0]!;
    run.user_artifacts = [];
    run.artifacts[0]!.data = { kind: "table", columns: ["Name"], rows: [["One"]] };
  });
  await expect.poll(() => table.screen.container.querySelector(".agent-line")).not.toBeNull();
  expect(table.screen.container.textContent).not.toContain("Next");
  expect(table.screen.container.textContent).not.toContain("Write this up as a note");
  await table.close();
});

test("an answer lifts as a reading page and is saved as its own Markdown under its title", async () => {
  const markdown = "Pin the lockfile first.\n\n## Why\n\n- `cargo update` moves every crate\n";
  const { screen, notes, close } = await workspace((state) => {
    state.executions[0]!.artifacts[0]!.data = { kind: "answer", markdown };
    state.executions[0]!.user_artifacts = [];
  });
  const cover = () => screen.container.querySelector<HTMLElement>('[data-card-id="result-card"]');
  await expect.poll(cover, { timeout: 5000 }).not.toBeNull();
  // A reviewed plan's result opens in the lift, where it is read and kept.
  cover()!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  cover()!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  const lift = screen.getByRole("dialog", { name: "Dependency findings" });
  await expect.element(lift).toBeVisible();
  // The header says what it is and what it answers; the page reads whole.
  await expect.poll(() => lift.element().querySelector(".lift-header")).not.toBeNull();
  const header = lift.element().querySelector(".lift-header")!;
  expect(header.querySelector(".kind")?.textContent).toBe("Answer");
  expect(header.querySelector(".meta")).toBeNull();
  expect(header.querySelector(".glyph")).toBeNull();
  const reading = lift.element().querySelector<HTMLElement>(".reading .answer.page")!;
  await expect.poll(() => reading.querySelector("h3")?.textContent).toBe("Why");
  expect(reading.querySelector("code")?.textContent).toBe("cargo update");
  expect(getComputedStyle(reading.parentElement!).maxInlineSize).toBe("640px");
  const copy = vi.spyOn(navigator.clipboard, "writeText").mockResolvedValue();
  await screen.getByRole("button", { name: "Copy", exact: true }).click();
  expect(copy).toHaveBeenCalledExactlyOnceWith(markdown);
  copy.mockRestore();
  await screen.getByRole("button", { name: "Save as note", exact: true }).click();
  await expect
    .element(screen.getByRole("button", { name: "Open note", exact: true }))
    .toBeVisible();
  const created = notes.calls.filter((call) => call.kind === "create");
  expect(created).toHaveLength(1);
  expect(created[0]).toMatchObject({
    kind: "create",
    markdown: `# Dependency findings\n\n${markdown.trim()}\n`,
  });
  await close();
});

test("a formatted document becomes Markdown under its title", () => {
  const markdown = documentMarkdown({
    key: "k",
    title: "Weekend in Lisbon",
    reviewLabel: "",
    evidence: [],
    content: {
      kind: "document",
      paragraphs: [],
      formatted: {
        version: 1,
        document: {
          type: "doc",
          content: [
            {
              type: "heading",
              attrs: { level: 1 },
              content: [{ type: "text", text: "Weekend in Lisbon" }],
            },
            {
              type: "paragraph",
              content: [
                { type: "text", text: "Stay in " },
                { type: "text", text: "Alfama", marks: [{ type: "bold" }] },
                { type: "text", text: ", see " },
                {
                  type: "text",
                  text: "the tram",
                  marks: [{ type: "link", attrs: { href: "https://carris.pt/28" } }],
                },
              ],
            },
            {
              type: "orderedList",
              attrs: { start: 1 },
              content: [
                {
                  type: "listItem",
                  content: [
                    { type: "paragraph", content: [{ type: "text", text: "Book *early*" }] },
                  ],
                },
                {
                  type: "listItem",
                  content: [{ type: "paragraph", content: [{ type: "text", text: "Walk" }] }],
                },
              ],
            },
          ],
        },
      },
    },
  });
  expect(markdown).toBe(
    "# Weekend in Lisbon\n\nStay in **Alfama**, see [the tram](https://carris.pt/28)\n\n1. Book \\*early\\*\n2. Walk\n",
  );
});
