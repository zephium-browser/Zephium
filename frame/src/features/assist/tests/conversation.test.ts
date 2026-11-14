import { expect, test } from "vitest";
import { render } from "svelte/server";
import type { PageAssistChangedV1, PageAssistEventV1 } from "$shared/ipc/bindings";
import StepsBlock from "../components/blocks/StepsBlock.svelte";
import { EMPTY, fold, latestReply, open, replyText, type Conversation } from "../lib/conversation";
import { parse } from "../lib/markdown";

test("typing approval renders the complete value and sites as text", () => {
  const value = `private  "quoted" <script>text</script> ${"x".repeat(180)} tail`;
  const label = `${JSON.stringify(value)} → Type into "Message"\nexample.com → ycombinator.com`;
  const { body } = render(StepsBlock, {
    props: {
      steps: [{ index: 0, label, category: "type", state: "confirm", reason: null }],
      site: "ycombinator.com",
      ondecide: () => {},
    },
  });
  expect(body).toContain(label.replaceAll("<", "&lt;"));
  expect(body).not.toContain("<script>");
});

const event = (seq: number, reply: number, kind: PageAssistEventV1): PageAssistChangedV1 => ({
  space: "s",
  conversation: "c1",
  reply,
  seq,
  kind,
});

const started: Conversation = {
  ...EMPTY,
  id: "c1",
  seq: 2,
  messages: [
    { kind: "person", id: 1, text: "What's worth reading?", suggestion: null, attachments: [] },
    {
      kind: "reply",
      id: 2,
      state: "working",
      phase: null,
      blocks: [],
      cost: null,
      fault: null,
      inert: false,
    },
  ],
};

function run(view: Conversation, ...events: PageAssistChangedV1[]): Conversation {
  return events.reduce((at, next) => {
    const folded = fold(at, next);
    if (folded === "reload" || folded === null) throw new Error(`did not fold ${next.seq}`);
    return folded;
  }, view);
}

test("text creates its block, grows it, and cites replace; blocks set by index", () => {
  const view = run(
    started,
    event(3, 2, { type: "text", block: 0, delta: "Three " }),
    event(4, 2, { type: "text", block: 0, delta: "stand out ⟦1⟧" }),
    event(5, 2, {
      type: "cites",
      block: 0,
      cites: [{ n: 1, excerpt: "eMudhra to exit", tab: "t1", host: "news.ycombinator.com" }],
    }),
    event(6, 2, { type: "block", block: 1, value: { kind: "opened", tabs: [] } }),
    event(7, 2, {
      type: "block",
      block: 1,
      value: {
        kind: "opened",
        tabs: [
          { index: 0, host: "github.com", url: "https://github.com", tab: null, state: "queued" },
        ],
      },
    }),
    event(8, 2, { type: "done", cost: { kind: "local", model: "m" } }),
  );
  const reply = latestReply(view)!;
  expect(reply.state).toBe("done");
  expect(open(reply)).toBe(false);
  expect(reply.blocks[0]).toMatchObject({ kind: "text", text: "Three stand out ⟦1⟧" });
  expect(reply.blocks[0]!.kind === "text" && reply.blocks[0]!.cites).toHaveLength(1);
  expect(reply.blocks).toHaveLength(2);
  expect(replyText(reply.blocks)).toBe("Three stand out");
  expect(view.seq).toBe(8);
});

test("an event already folded is skipped; a gap or a stranger reply goes back to the snapshot", () => {
  expect(fold(started, event(2, 2, { type: "phase", phase: { phase: "thinking" } }))).toBeNull();
  expect(fold(started, event(4, 2, { type: "phase", phase: { phase: "thinking" } }))).toBe(
    "reload",
  );
  expect(fold(started, event(3, 9, { type: "cancelled" }))).toBe("reload");
  expect(fold(started, { ...event(3, 2, { type: "cancelled" }), conversation: "c2" })).toBeNull();
  expect(fold(started, { ...event(0, 0, { type: "invalidated" }) })).toBe("reload");
});

test("inert reaches every reply up to the one named; titled names the conversation", () => {
  const view = run(
    started,
    event(3, 0, { type: "titled", title: "Worth reading" }),
    event(4, 2, { type: "inert" }),
  );
  expect(view.title).toBe("Worth reading");
  expect(latestReply(view)!.inert).toBe(true);
});

test("markdown never becomes markup, keeps only web links, and turns marks into cites", () => {
  const tree = parse("Read **this** ⟦2⟧ <b>x</b> [a](javascript:alert(1)) [b](https://b.com)");
  const paragraph = tree[0]!;
  expect(paragraph.kind).toBe("paragraph");
  const nodes = paragraph.kind === "paragraph" ? paragraph.children : [];
  expect(nodes.some((node) => node.kind === "cite" && node.n === 2)).toBe(true);
  expect(nodes.some((node) => node.kind === "text" && node.text === "<b>")).toBe(true);
  expect(
    nodes.filter((node) => node.kind === "link").map((node) => node.kind === "link" && node.href),
  ).toEqual(["https://b.com"]);
});

test("the words the latest deltas brought are split off the end to ink in", () => {
  const tree = parse("One two three", [
    { at: 0, length: 8 },
    { at: 8, length: 5 },
  ]);
  const nodes = tree[0]!.kind === "paragraph" ? tree[0]!.children : [];
  expect(nodes).toEqual([
    { kind: "fresh", text: "One two ", key: 0 },
    { kind: "fresh", text: "three", key: 8 },
  ]);
});
