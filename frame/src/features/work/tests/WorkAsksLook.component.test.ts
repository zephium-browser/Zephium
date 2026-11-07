import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page } from "vitest/browser";
import { asksOf } from "../components/asks/asks";
import AsksSheet from "./AsksSheet.svelte";
import Remembered from "../components/asks/Remembered.svelte";
import * as f from "./ask-fixtures";

const FRAME = new URL("./fixtures/ask-page-frame.svg", import.meta.url).href;
vi.mock("$domain/resources", async (original) => ({
  ...(await original<typeof import("$domain/resources")>()),
  pageFrameUrl: () => FRAME,
}));
vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  const memories: import("$shared/ipc/bindings").WorkMemoryV1[] = [
    {
      id: "01M3F2YJTVB6S1M73MERQER7NA",
      text: "Prefers aisle seats on long flights",
      kind: "preference",
      work: "01M3CV1H7HRABAGVH1T8HXH2DD",
      created_ms: "1790000000000",
    },
    {
      id: "01M3F2YJTVB6S1M73MERQER7NB",
      text: "Budget for the stay is about $4,000 a month",
      kind: "preference",
      work: "01M3CV1H7HRABAGVH1T8HXH2DD",
      created_ms: "1790000000000",
    },
  ];
  return mockBindings({
    faviconProbe: async () => true,
    workMemories: async (profile: string) => ({
      version: 1,
      profile,
      memories,
      refused: null,
      error: null,
    }),
  });
});

const shots = "../../../../../target/work-asks";
const settle = (ms = 500) => new Promise((done) => setTimeout(done, ms));
const noop = {
  confirm: async () => true,
  answer: async () => true,
  openPage: () => {},
  signedIn: async () => true,
  chooseFolder: async () => null,
};

function askRows() {
  const execution = f.runWith([
    f.slackTask,
    f.slackEntry,
    f.slackSend,
    f.airbnbBook,
    f.notionEdit,
    f.typeSearch,
    f.deleteRepo,
    f.historyAsk,
    f.notesAsk,
    f.tabsAsk,
    f.githubAsk,
    f.folderAsk,
    f.documentsAsk,
    f.addressAsk,
    f.budgetAsk,
    f.notionTask,
    f.settled(f.decided(f.slackSend, "approved", "succeeded"), { id: "sent" }),
    f.settled(f.decided(f.airbnbBook, "declined", "cancelled"), { id: "declined" }),
    f.settled(f.decided(f.slackSend, "approved", "running"), { id: "sending" }),
    f.settled(
      f.decided(
        f.slackSend,
        "approved",
        "failed",
        "The page changed before it ran; nothing was sent",
      ),
      { id: "changed" },
    ),
    f.settled(f.answered(f.slackEntry, "Always for Slack"), { id: "entry-always" }),
    f.settled(f.answered(f.historyAsk, "Allow"), { id: "history-allowed" }),
    f.settled(f.answered(f.folderAsk, "Allow for this work"), { id: "folder-allowed" }),
    f.settled(f.answered(f.addressAsk, "Don\u2019t open"), { id: "address-declined" }),
    f.settled(f.answered(f.documentsAsk, "/Users/crynta/Documents/Notes"), {
      id: "documents-chosen",
    }),
  ]);
  const asks = asksOf(execution, [f.tripPage], [f.notionWall]);
  return asks.map((ask) => ({ name: `${ask.kind}-${ask.step}`, ask }));
}

// Each retained image has its own test budget. One loop of all rows in both
// themes exceeded 15 seconds on CI and cancelled screenshot work mid-frame.
for (const theme of ["dark", "light"]) {
  for (const row of askRows()) {
    test(`${row.name}, on the canvas and in the island, ${theme}`, async () => {
      await page.viewport(1500, 900);
      document.documentElement.dataset.theme = theme;
      const screen = await render(AsksSheet, { rows: [row], actions: noop });
      try {
        const element = screen.container.querySelector<HTMLElement>(".row")!;
        if (row.ask.kind === "confirm" && row.ask.state === "open" && row.ask.frame) {
          const frames = [...element.querySelectorAll<HTMLCanvasElement>(".page canvas")];
          expect(frames).toHaveLength(3);
          for (const canvas of frames) {
            await expect
              .poll(() => [...canvas.getContext("2d")!.getImageData(1, 1, 1, 1).data])
              .toEqual([214, 226, 240, 255]);
          }
        }
        await page
          .elementLocator(element)
          .screenshot({ path: `${shots}/${row.name}-${theme}.png` });
      } finally {
        document.documentElement.dataset.theme = "dark";
        await screen.unmount();
      }
    });
  }
}

test("what a run remembered, on the canvas with Undo, in both themes", async () => {
  await page.viewport(700, 200);
  const screen = await render(Remembered, {
    profile: "00000000000000000000000001",
    work: "01M3CV1H7HRABAGVH1T8HXH2DD",
  });
  const list = screen.container.parentElement!;
  list.style.padding = "32px";
  list.style.background = "var(--color-canvas)";
  list.style.inlineSize = "560px";
  await settle(900);
  for (const theme of ["dark", "light"]) {
    document.documentElement.dataset.theme = theme;
    await settle();
    await page.elementLocator(list).screenshot({ path: `${shots}/remembered-${theme}.png` });
  }
  document.documentElement.dataset.theme = "dark";
  await screen.unmount();
});
