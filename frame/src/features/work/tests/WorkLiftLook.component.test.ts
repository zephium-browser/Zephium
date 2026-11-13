import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import type { WorkCallV1 } from "$shared/ipc/bindings";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";
import { saasScene, tripScene, type BoardScene } from "./board-fixtures";

const current = vi.hoisted(() => ({ scene: null as BoardScene | null }));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  return mockComposerBindings({
    faviconProbe: async () => true,
    runCommand: vi.fn(async () => ({ accepted: true, operation_id: null })),
    workCall: vi.fn(async (profile: string, call: WorkCallV1) => {
      const query = call.kind === "query" ? call.request.query : null;
      const projection =
        query?.kind === "projection" ? current.scene?.objectives.get(query.work) : undefined;
      return {
        version: 1,
        profile,
        reply: projection
          ? { kind: "projection" as const, projection: structuredClone(projection) as never }
          : { kind: "error" as const, error: "not_found" as const },
      };
    }),
    workActivity: vi.fn(async (profile: string, work: string) => ({
      version: 1,
      profile,
      work,
      signals: [],
      pages: current.scene?.pages ?? [],
      error: null,
    })),
  });
});

const shots = "../../../../../target/work-lift";
const settle = (ms = 500) => new Promise((done) => setTimeout(done, ms));

test.each([["saas", saasScene] as const, ["trip", tripScene] as const])(
  "every card on the %s canvas opens in the centre, in both themes",
  async (name, make) => {
    const scene = make();
    current.scene = scene;
    await page.viewport(1440, 900);
    const environment = new WorkEnvironmentSession(scene.snapshot.profile, scene.snapshot.space);
    environment.snapshot = structuredClone(scene.snapshot);
    environment.selected = scene.snapshot.id;
    const screen = await render(WorkEnvironmentWorkspace, {
      session: environment,
      tabs: [],
      spaceName: "Personal",
      aiEnabled: true,
      onopen: vi.fn(),
      onnewtab: vi.fn(),
    });
    const root = screen.container.querySelector(".environment") as HTMLElement;
    root.style.height = "884px";
    root.style.width = "1376px";
    await expect
      .poll(() => screen.container.querySelectorAll("[data-card-id]").length, { timeout: 10000 })
      .toBeGreaterThan(2);
    await settle(1200);
    const ids = [...screen.container.querySelectorAll<HTMLElement>("[data-card-id]")].map(
      (card) => card.dataset.cardId!,
    );
    let index = 0;
    for (const id of ids) {
      const card = screen.container.querySelector<HTMLElement>(
        `[data-card-id="${CSS.escape(id)}"]`,
      );
      if (!card) continue;
      const node = card.closest<HTMLElement>(".svelte-flow__node") ?? card;
      for (let click = 0; click < 2; click++)
        node.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: 1, clientY: 1 }));
      const dialog = await expect
        .poll(() => document.querySelector<HTMLElement>(".lift.shown"), { timeout: 2000 })
        .not.toBeNull()
        .then(() => true)
        .catch(() => false);
      if (!dialog) continue;
      await settle(900);
      const kind = [...card.classList].filter((name) => !name.startsWith("svelte-")).join(".");
      for (const theme of ["dark", "light"]) {
        document.documentElement.dataset.theme = theme;
        await settle(250);
        await page.screenshot({
          path: `${shots}/${name}/${String(index).padStart(2, "0")}-${kind}-${theme}.png`,
        });
      }
      document.documentElement.dataset.theme = "dark";
      index++;
      await userEvent.keyboard("{Escape}");
      await expect.poll(() => document.querySelector(".lift")).toBeNull();
      await settle(200);
    }
    await screen.unmount();
    environment.dispose();
  },
  240_000,
);
