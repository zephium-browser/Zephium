import "$styles/global.css";
import { expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import { WorkEnvironmentSession } from "$domain/work-environment";
import { tabFixture } from "$shared/testing/fixtures";
import WorkEnvironmentWorkspace from "../components/WorkEnvironmentWorkspace.svelte";

vi.mock("$shared/ipc/bindings", async () => {
  const { mockComposerBindings } = await import("./composer-fixtures");
  const { models } = await import("$shared/testing/work-models");
  return mockComposerBindings({
    workModels: vi.fn(async (profile: string) => ({
      ...models({ keys: { anthropic: "valid" }, lead: "anthropic/claude-opus-5-5" }),
      profile,
    })),
    faviconProbe: async () => true,
    runCommand: vi.fn(async () => ({ accepted: true, operation_id: null })),
    workCall: vi.fn(async (profile: string) => ({
      version: 1,
      profile,
      reply: { kind: "error" as const, error: "not_found" as const },
    })),
  });
});

const shots = "../../../../../target/work-polish";
const settle = () => new Promise((done) => setTimeout(done, 500));

test("the bar's own controls: composer, + menu and the model chip, both themes", async () => {
  const { projection, snapshot } = await import("./environment-fixtures");
  await page.viewport(1440, 900);
  const environment = new WorkEnvironmentSession(snapshot.profile, snapshot.space);
  environment.snapshot = { ...structuredClone(snapshot), title: "YC trip from Warsaw" };
  environment.selected = snapshot.id;
  void projection;
  const screen = await render(WorkEnvironmentWorkspace, {
    session: environment,
    tabs: [
      tabFixture({
        id: "t1",
        title: "Y Combinator: apply for the next batch",
        url: "https://www.ycombinator.com/apply",
      }),
      tabFixture({
        id: "t2",
        title: "Flights Warsaw to San Francisco",
        url: "https://www.google.com/travel/flights",
      }),
      tabFixture({
        id: "t3",
        title: "Airbnb: stays in San Francisco",
        url: "https://www.airbnb.com/s/San-Francisco",
      }),
    ],
    spaceName: "Personal",
    aiEnabled: true,
    onopen: vi.fn(),
    onnewtab: vi.fn(),
  });
  const root = screen.container.querySelector(".environment") as HTMLElement;
  root.style.height = "884px";
  root.style.width = "1376px";
  const shoot = async (name: string) => {
    for (const theme of ["dark", "light"]) {
      document.documentElement.dataset.theme = theme;
      await settle();
      await page.screenshot({ path: `${shots}/${name}-${theme}.png` });
    }
    document.documentElement.dataset.theme = "dark";
  };
  await shoot("rest");
  const field = screen.getByRole("textbox", { name: "What do you want to do?" });
  await field.click();
  await userEvent.keyboard("Plan my trip to the next YC batch from Warsaw");
  await expect
    .poll(() => screen.container.querySelector<HTMLButtonElement>(".model-trigger")?.disabled)
    .toBe(false);
  await shoot("typing");
  await screen.getByRole("button", { name: "Add", exact: false }).first().click();
  await expect.element(page.getByRole("group", { name: "Add" })).toBeVisible();
  await shoot("plus-tabs");
  await page.getByRole("button", { name: "Links" }).click();
  await shoot("plus-link");
  await page.getByRole("button", { name: "Folders" }).click();
  await shoot("plus-folder");
  await userEvent.keyboard("{Escape}");
  await screen.container.querySelector<HTMLButtonElement>(".model-trigger")!.click();
  await shoot("picker");
  await userEvent.keyboard("{Escape}");
  await screen.unmount();
  environment.dispose();
});
