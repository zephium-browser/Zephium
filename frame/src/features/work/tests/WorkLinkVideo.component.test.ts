import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type { WorkCallV1, WorkEnvironmentSnapshot } from "$shared/ipc/bindings";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";

const native = vi.hoisted(() => ({
  call: vi.fn(),
  admit: vi.fn(),
  resource: vi.fn(),
  paneShow: vi.fn(),
  settle: vi.fn(),
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({
    faviconProbe: async () => true,
    workCall: native.call,
    mediaAdmitRemote: native.admit,
    linkDescribe: async () => ({ status: "ok" as const, data: true }),
    resourceCall: native.resource,
    workPaneShow: native.paneShow,
    workPaneHide: vi.fn(),
    workPaneSetRect: vi.fn(),
  });
});
vi.mock("$domain/operations", () => ({ settle: native.settle }));
// The admitted thumbnail as a picture the test page can draw.
const PIXEL =
  "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";
vi.mock("$domain/resources", async (original) => ({
  ...(await original<typeof import("$domain/resources")>()),
  mediaUrl: (_profile: string, digest: string) => `${PIXEL}#${digest}`,
}));

const profile = "00000000000000000000000001";
const video = "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42";

/** The admitted thumbnail's record, as the resource store answers for it. */
const thumbnailRecord = {
  kind: "record",
  record: {
    id: "thumbnail",
    revision: "1",
    created_at: "0",
    updated_at: "0",
    trashed: false,
    draft: {
      title: "hqdefault.jpg",
      pinned: false,
      related: [],
      content: {
        kind: "media",
        asset: {
          version: 1,
          kind: "image",
          mime: "image/jpeg",
          bytes: 4096,
          digest: "e".repeat(64),
          name: "hqdefault.jpg",
          origin: {
            kind: "fetched",
            url: "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg",
            observed_at: "2026-09-24",
          },
        },
      },
    },
  },
};

async function workspace(pictured: boolean) {
  await page.viewport(1200, 800);
  const snapshot: WorkEnvironmentSnapshot = {
    version: 1,
    profile,
    id: "00000000000000000000000003",
    space: "00000000000000000000000002",
    title: "Videos",
    revision: "1",
    lifecycle: "active",
    areas: [],
    view: {
      revision: "1",
      x: 0,
      y: 0,
      zoom_milli: 1000,
      placements: [{ element: "link", x: 80, y: 80, width: 248, height: 120 }],
    },
    elements: [
      { id: "link", area: null, reference: { kind: "link", url: video, title: "youtube.com" } },
      ...(pictured
        ? [
            {
              id: "picture",
              area: null,
              reference: { kind: "resource" as const, resource: "thumbnail" },
            },
          ]
        : []),
    ],
    ...(pictured
      ? {
          relations: [
            {
              id: "uses",
              from: "link",
              to: "picture",
              kind: "uses" as const,
              origin: { kind: "user" as const },
            },
          ],
        }
      : {}),
  };
  const environment = new WorkEnvironmentSession(profile, snapshot.space);
  environment.snapshot = snapshot;
  environment.selected = snapshot.id;
  native.call.mockImplementation(async (_profile: string, _call: WorkCallV1) => ({
    version: 1,
    profile,
    reply: { kind: "environment", reply: { kind: "snapshot", snapshot: environment.snapshot! } },
  }));
  native.resource.mockImplementation(async (_profile: string, call: { kind: string }) => ({
    profile,
    response: call.kind === "get" ? thumbnailRecord : { kind: "page", items: [], next: null },
  }));
  native.admit.mockImplementation(
    async (_profile: string, _environment: string, element: string) => {
      const current = environment.snapshot!;
      environment.snapshot = {
        ...current,
        revision: String(BigInt(current.revision) + 1n),
        elements: [
          ...current.elements,
          { id: "picture", area: null, reference: { kind: "resource", resource: "thumbnail" } },
        ],
        relations: [
          { id: "uses", from: element, to: "picture", kind: "uses", origin: { kind: "user" } },
        ],
      };
      return { status: "ok", data: { kind: "admitted", element: "picture" } };
    },
  );
  native.paneShow.mockResolvedValue({ accepted: true, operation_id: "0000000000000001" });
  native.settle.mockResolvedValue({ outcome: "applied", disposition: null });
  const screen = await render(WorkEnvironmentWorkspace, {
    session: environment,
    tabs: [],
    spaceName: "Personal",
    aiEnabled: false,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "720px";
  root.style.width = "1100px";
  return { screen, environment };
}

// The thumbnail is the video's poster, never a media card of its own.
// The picture waits on a notes lookup before its media read.
const posterOf = (container: HTMLElement) =>
  container.querySelector<HTMLImageElement>(".media.video img")?.getAttribute("src") ?? "";

test("a video link draws its admitted thumbnail as its poster and opens its page", async () => {
  native.admit.mockClear();
  const { screen, environment } = await workspace(true);
  await expect
    .poll(() => posterOf(screen.container), { timeout: 5000 })
    .toBe(`${PIXEL}#${"e".repeat(64)}`);
  expect(screen.container.textContent).not.toContain("hqdefault.jpg");
  // One picture is enough: a pictured link asks for nothing more.
  expect(native.admit).not.toHaveBeenCalled();
  // A click opens its page like any link: nothing plays in the canvas.
  (screen.container.querySelector(".media.video button") as HTMLElement).click();
  await expect.poll(() => native.paneShow.mock.calls.length).toBeGreaterThan(0);
  expect(screen.container.querySelector("iframe")).toBeNull();
  await screen.unmount();
  environment.dispose();
});

test("a pasted video link asks for the poster its video host serves, once, and draws it", async () => {
  native.admit.mockClear();
  const { screen, environment } = await workspace(false);
  await expect.poll(() => native.admit.mock.calls.length, { timeout: 5000 }).toBe(1);
  expect(native.admit.mock.calls[0]!.slice(1)).toEqual([
    "00000000000000000000000003",
    "link",
    "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg",
  ]);
  await expect
    .poll(() => posterOf(screen.container), { timeout: 5000 })
    .toBe(`${PIXEL}#${"e".repeat(64)}`);
  // A pasted link is stored with its host for a name: the card says what it is instead.
  expect(screen.container.querySelector(".media.video figcaption")?.textContent).toContain(
    "YouTube video",
  );
  await new Promise((settled) => setTimeout(settled, 300));
  expect(native.admit).toHaveBeenCalledTimes(1);
  await screen.unmount();
  environment.dispose();
});
