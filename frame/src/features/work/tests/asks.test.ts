import { describe, expect, test } from "vitest";
import { askPart, asksOf, confirmVerb, openAsks } from "../components/asks/asks";
import * as f from "./ask-fixtures";

describe("asks", () => {
  test("a folder the run needs is chosen in the panel, with the agent's reason", () => {
    const [ask] = asksOf(f.runWith([f.documentsAsk]));
    expect(ask).toMatchObject({
      kind: "folder",
      name: "Documents",
      choose: true,
      reason: "To save binary-search.md there.",
      decline: "Not now",
    });
    const [named] = asksOf(f.runWith([f.folderAsk]));
    expect(named).toMatchObject({ kind: "folder", choose: false, reason: null });
  });

  test("an address the agent wrote reads with its host and Rust's own option words", () => {
    const [ask] = asksOf(f.runWith([f.addressAsk]));
    expect(ask).toMatchObject({
      kind: "address",
      host: "warsaw-sfo-flights.collector.example",
      open: "Open",
      allowSite: "Allow collector.example for this request",
      decline: "Don\u2019t open",
    });
    const plain = { ...f.addressAsk, kind: { ...f.addressAsk.kind, prompt: "Open it?" } };
    expect(asksOf(f.runWith([plain as typeof f.addressAsk]))[0]?.kind).toBe("question");
  });

  test("typing held on a site the person did not name reads as Type with the run option", () => {
    const [ask] = asksOf(f.runWith([f.typeSearch]));
    expect(ask).toMatchObject({ kind: "confirm", verb: "Type", runOption: true });
  });

  test("a held step reads as a Confirm with the page's own words and the page's frame", () => {
    const [ask] = asksOf(f.runWith([f.airbnbBook]), [f.tripPage]);
    expect(ask).toMatchObject({
      kind: "confirm",
      step: "confirm-airbnb",
      state: "open",
      verb: "Request to book",
      headline: "Request to book for $4,212?",
      url: f.tripPage.url,
    });
    expect(ask?.kind === "confirm" && ask.frame).toContain("frame/01M3CV1H7HRABAGVH1T8HXH2DD/");
  });

  test("a decision moves a Confirm through working to its receipt", () => {
    const states = [
      f.decided(f.slackSend, "approved", "running"),
      f.decided(f.slackSend, "approved", "succeeded"),
      f.decided(f.slackSend, "declined", "cancelled"),
      f.decided(
        f.slackSend,
        "approved",
        "failed",
        "The page changed before it ran; nothing was sent",
      ),
      f.decided(f.slackSend, "approved", "outcome_unknown"),
      f.settled(f.slackSend, { status: "cancelled" }),
    ].map((step) => asksOf(f.runWith([step]))[0]?.state);
    expect(states).toEqual(["working", "done", "declined", "failed", "unknown", "gone"]);
  });

  test("Rust's fixed questions become their own cards; anything else is the agent's question", () => {
    const asks = asksOf(
      f.runWith([
        f.slackTask,
        f.slackEntry,
        f.historyAsk,
        f.notesAsk,
        f.tabsAsk,
        f.githubAsk,
        f.budgetAsk,
      ]),
    );
    expect(asks.map((ask) => ask.kind)).toEqual([
      "entry",
      "context",
      "context",
      "context",
      "connection",
      "question",
    ]);
    expect(asks[0]).toMatchObject({
      name: "Slack",
      host: "app.slack.com",
      plan: "Read #design since Monday and draft a reply to Anna",
      always: "Always for Slack",
    });
    expect(asks[1]).toMatchObject({
      source: "history",
      reason: "Looking for the flight comparison you read last week.",
    });
    expect(asks[4]).toMatchObject({ service: "GitHub", tool: "gh", host: "github.com" });
  });

  test("one entry question for several services carries each service's host", () => {
    const [ask] = asksOf(f.runWith([...f.dayTasks, f.dayEntry]));
    expect(ask).toMatchObject({
      kind: "entry",
      name: "Slack, Gmail and Calendar",
      host: null,
      hosts: ["app.slack.com", "mail.google.com", "calendar.google.com"],
    });
  });

  test("the runtime's purpose decides the card; words only for runs from before it", () => {
    const said = (step: typeof f.slackEntry, purpose: string, patch: object = {}) =>
      f.settled(step, { kind: { ...step.kind, ...patch, purpose } } as never);
    const asks = asksOf(
      f.runWith([
        f.slackTask,
        said(f.slackEntry, "entry", { options: ["Allow", "Not now"] }),
        said(f.historyAsk, "context", { prompt: "May I look at your browsing history for it?" }),
        said(f.githubAsk, "connection"),
        said(f.budgetAsk, "budget"),
        // Words that read like a site's entry, from the agent's own question.
        said(f.slackEntry, "question"),
      ]),
    );
    expect(asks.map((ask) => ask.kind)).toEqual([
      "entry",
      "context",
      "connection",
      "question",
      "question",
    ]);
    expect(asks[0]).toMatchObject({ name: "Slack", always: "Always for Slack" });
    expect(asks[1]).toMatchObject({ source: "history" });
  });

  test("a sign-in wall on a page task is an ask on that page's host", () => {
    const asks = asksOf(f.runWith([f.notionTask]), [], [f.notionWall]);
    expect(asks).toMatchObject([{ kind: "sign_in", host: "notion.so", state: "open" }]);
    expect(asksOf(f.runWith([f.notionTask]), [], [{ ...f.notionWall, phase: "released" }])).toEqual(
      [],
    );
  });

  test("open asks lead with the newest and leave receipts behind", () => {
    const asks = asksOf(f.runWith([f.historyAsk, f.answered(f.notesAsk, "Allow"), f.slackSend]));
    expect(openAsks(asks).map((ask) => ask.step)).toEqual(["confirm-slack", "ask-history"]);
  });

  test("buttons say the page's control when it is a name, else the kind's verb", () => {
    expect(confirmVerb("communication", "press Send")).toBe("Send");
    expect(confirmVerb("communication", "press Enter in Message #design")).toBe("Send");
    expect(confirmVerb("edit", "type into Page")).toBe("Save");
    expect(confirmVerb("purchase", "press Confirm and pay with the card ending 4242")).toBe("Book");
  });

  test("an ask stands on its lead part, else on the part of its site", () => {
    const asks = asksOf(f.runWith([f.slackTask, f.slackEntry, f.airbnbBook, f.budgetAsk]));
    expect(asks.map(askPart)).toEqual(["slack.com", "airbnb.co.uk", null]);
    expect(askPart({ ...asks[2]!, part: "01PART" })).toBe("01PART");
  });
});
