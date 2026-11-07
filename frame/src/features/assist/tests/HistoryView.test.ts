import { expect, test } from "vitest";
import { render } from "svelte/server";
import HistoryView from "../components/HistoryView.svelte";
import type { AssistSession } from "../lib/assist-session.svelte";

test.each([{ past: [] }, { past: [{ id: "past", title: "Past conversation", updated_ms: 0 }] }])(
  "history discloses retention after the kept conversations",
  ({ past }) => {
    const session = { past, conversation: { id: "current" } } as unknown as AssistSession;
    const { body } = render(HistoryView, { props: { session, onback: () => {} } });
    expect(body).toContain("Zephium keeps your 200 most recent conversations.");
    if (past.length)
      expect(body.indexOf("Past conversation")).toBeLessThan(body.indexOf("Zephium keeps"));
  },
);
