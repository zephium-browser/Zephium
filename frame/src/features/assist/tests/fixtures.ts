import type {
  PageAssistBlockV1,
  PageAssistChangedV1,
  PageAssistEventV1,
  PageAssistMessageV1,
  PageAssistStatusV1,
} from "$shared/ipc/bindings";

export const SPACE = "space-1";
export const TAB = "tab-1";
export const CONVERSATION = "c1";

export const status = (
  messages: PageAssistMessageV1[] = [],
  patch: Partial<PageAssistStatusV1> = {},
): PageAssistStatusV1 => ({
  version: 2,
  space: SPACE,
  tab: TAB,
  availability: "ready",
  page: { tab: TAB, host: "news.ycombinator.com", title: "Hacker News" },
  model: { id: "openai/gpt-6-luna", name: "GPT-6 Luna", hosted: false, effort: "high" },
  suggestions: ["summarize", "picks", "key_points", "redesign", "darker"],
  selection: null,
  can_pick: true,
  styled: false,
  conversation: {
    id: CONVERSATION,
    title: messages.length ? "Worth reading" : null,
    messages,
    seq: 40,
  },
  ...patch,
});

export const person = (
  id: number,
  text: string,
  attachments: Extract<PageAssistMessageV1, { kind: "person" }>["attachments"] = [
    { kind: "page", tab: TAB, host: "news.ycombinator.com", title: "Hacker News" },
  ],
): PageAssistMessageV1 => ({ kind: "person", id, text, suggestion: null, attachments });

export const reply = (
  id: number,
  blocks: PageAssistBlockV1[],
  patch: Partial<Extract<PageAssistMessageV1, { kind: "reply" }>> = {},
): PageAssistMessageV1 => ({
  kind: "reply",
  id,
  state: "done",
  phase: null,
  blocks,
  cost: { kind: "byok", model: "GPT-6 Luna", provider: "OpenAI", usd: "0.004" },
  fault: null,
  inert: false,
  end: null,
  ...patch,
});

export const text = (words: string, cites: number[] = []): PageAssistBlockV1 => ({
  kind: "text",
  text: words,
  cites: cites.map((n) => ({
    n,
    excerpt: `Passage ${n} as the page says it`,
    tab: TAB,
    host: "news.ycombinator.com",
  })),
});

export const ITEMS: PageAssistBlockV1 = {
  kind: "items",
  tab: TAB,
  items: [
    {
      n: 1,
      title: "eMudhra to exit the root store after a mis-issuance",
      meta: ["#2", "749 points", "541 comments"],
      url: "https://emudhra.com/",
      marked: true,
    },
    {
      n: 2,
      title: "Show HN: A tiny SQLite extension for vector search",
      meta: ["#5", "412 points", "96 comments"],
      url: "https://github.com/example/vec",
      marked: true,
    },
    {
      n: 3,
      title: "The case for writing your own CSS reset",
      meta: ["#9", "233 points", "180 comments"],
      url: "https://example.com/reset",
      marked: false,
    },
  ],
};

export const OPENED: PageAssistBlockV1 = {
  kind: "opened",
  tabs: [
    { index: 0, host: "emudhra.com", url: "https://emudhra.com/", tab: "t-new-1", state: "opened" },
    {
      index: 1,
      host: "github.com",
      url: "https://github.com/example/vec",
      tab: null,
      state: "opening",
    },
    { index: 2, host: "example.com", url: "https://example.com/reset", tab: null, state: "queued" },
    {
      index: 3,
      host: "blocked.example",
      url: "https://blocked.example/",
      tab: null,
      state: "failed",
    },
  ],
};

export const HANDOFF = (
  state: Extract<PageAssistBlockV1, { kind: "handoff" }>["handoff"]["state"] = { state: "offered" },
): PageAssistBlockV1 => ({
  kind: "handoff",
  handoff: {
    title: "Set up status.zephium.app",
    summary: "Create the status page, wire the release checks and open a pull request for review.",
    brings: [
      { kind: "task", label: "Set up status.zephium.app", detail: "This week", tab: null },
      { kind: "repository", label: "zephium-browser/Zephium", detail: "Releases", tab: null },
      { kind: "folder", label: "~/Dev/zephium-infra", detail: "Local folder", tab: null },
      { kind: "tab", label: "Status page docs", detail: "Open tab", tab: TAB },
      { kind: "conversation", label: "This conversation", detail: null, tab: null },
    ],
    state,
  },
});

let seq = 40;
export const resetSeq = (to = 40) => {
  seq = to;
};

/** The next event of the conversation, in order. */
export const event = (reply: number, kind: PageAssistEventV1): PageAssistChangedV1 => ({
  space: SPACE,
  conversation: CONVERSATION,
  reply,
  seq: ++seq,
  kind,
});
