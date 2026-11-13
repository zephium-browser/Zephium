import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type { WorkCallV1, WorkEnvironmentSnapshot } from "$shared/ipc/bindings";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
const native = vi.hoisted(() => ({
  call: vi.fn(),
  admitFolder: vi.fn(),
  reveal: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({
    workCall: native.call,
    workAdmitFolder: native.admitFolder,
    workRevealPath: native.reveal,
  });
});

test("a Finder drop admits its folders, keeps quiet about the files beside them, and the card reveals where it lives", async () => {
  await page.viewport(1200, 800);
  const profile = "00000000000000000000000001";
  const space = "00000000000000000000000002";
  let snapshot: WorkEnvironmentSnapshot = {
    version: 1,
    id: "00000000000000000000000003",
    profile,
    space,
    title: "Files",
    lifecycle: "active",
    revision: "1",
    elements: [],
    areas: [],
    view: { revision: "1", x: 0, y: 0, zoom_milli: 1000, placements: [] },
  };
  native.call.mockImplementation(async (_profile: string, call: WorkCallV1) => {
    if (call.kind !== "environment") throw new Error("Folders never start execution");
    const request = call.request;
    if (request.kind === "checkpoint") {
      snapshot = {
        ...snapshot,
        view: { ...request.view, revision: String(BigInt(request.expected) + 1n) },
      };
      return {
        version: 1,
        profile,
        reply: {
          kind: "environment",
          reply: {
            kind: "checkpointed",
            expected: request.expected,
            applied_view_revision: snapshot.view.revision,
            replayed: false,
            snapshot,
          },
        },
      };
    }
    if (request.kind === "command") {
      if (request.intent.kind === "edit" && request.intent.edit.kind === "add")
        snapshot = {
          ...snapshot,
          revision: String(snapshot.elements.length + 2),
          elements: [
            ...snapshot.elements,
            {
              id: `element-${snapshot.elements.length}`,
              area: null,
              reference: request.intent.edit.reference,
            },
          ],
        };
      return {
        version: 1,
        profile,
        reply: {
          kind: "environment",
          reply: {
            kind: "applied",
            command: request.command,
            applied_revision: snapshot.revision,
            applied_view_revision: snapshot.view.revision,
            replayed: false,
            snapshot,
          },
        },
      };
    }
    return {
      version: 1,
      profile,
      reply: { kind: "environment", reply: { kind: "snapshot", snapshot } },
    };
  });
  native.admitFolder.mockImplementation(async (_profile: string, path: string) =>
    path.endsWith(".pdf")
      ? { status: "ok", data: { kind: "refused", not_a_folder: true } }
      : { status: "ok", data: { kind: "admitted", path, name: "Papers" } },
  );
  native.reveal.mockResolvedValue({ status: "ok", data: true });
  const session = new WorkEnvironmentSession(profile, space);
  session.snapshot = snapshot;
  session.selected = snapshot.id;
  const screen = await render(WorkEnvironmentWorkspace, {
    session,
    tabs: [],
    spaceName: "Personal",
    aiEnabled: false,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "720px";
  root.style.width = "1100px";
  window.dispatchEvent(
    new CustomEvent("zephium:work-paths-dropped", {
      detail: { paths: ["/Users/reader/Papers", "/Users/reader/Papers/notes.pdf"], x: 420, y: 300 },
    }),
  );
  await expect
    .poll(() => session.snapshot?.elements.map((element) => element.reference))
    .toEqual([{ kind: "folder", path: "/Users/reader/Papers", name: "Papers" }]);
  expect(native.admitFolder).toHaveBeenCalledTimes(2);
  // A file among the dropped paths is not a refusal: its path joins the request being written.
  expect(screen.container.textContent).not.toContain("This folder can’t be used");
  await expect.poll(() => session.composer).toBe("/Users/reader/Papers/notes.pdf");
  await expect
    .poll(() => screen.container.querySelectorAll(".work-drag-handle").length, {
      timeout: 5000,
    })
    .toBe(1);
  await screen.container.querySelector<HTMLElement>(".work-drag-handle")!.click();
  await screen.getByRole("button", { name: "Open", exact: true }).click();
  await screen.getByRole("button", { name: "Show in Folder", exact: true }).click();
  await expect.poll(() => native.reveal.mock.calls).toEqual([[profile, "/Users/reader/Papers"]]);
  await screen.unmount();
  session.dispose();
});
