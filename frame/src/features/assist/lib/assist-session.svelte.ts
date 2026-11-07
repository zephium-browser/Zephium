import {
  commands,
  type PageAssistAttachV1,
  type PageAssistChangedV1,
  type PageAssistCiteV1,
  type PageAssistDecisionV1,
  type PageAssistFaultV1,
  type PageAssistPastV1,
  type PageAssistPickV1,
  type PageAssistStatusV1,
  type PageAssistSuggestionV1,
  type PageStylesV1,
} from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { EMPTY, askedBy, fold, latestReply, open, type Conversation } from "./conversation";
import type { Arrival } from "./markdown";

/** Rust holds the same bound, in UTF-8 bytes; a character is at most four. */
export const MESSAGE_LIMIT = 2048;
/** The latest deltas of a streaming text that still ink in. */
const FRESH = 3;
const MAX_ATTACHMENTS = 6;
const MAX_OTHERS = 3;

/** A picked element, with the address its page had when it was picked. */
export type Pick = PageAssistPickV1 & { url: string | null };

const quietly = <T>(promise: Promise<T>, fallback: T) => promise.catch(() => fallback);

/**
 * The panel's view of a Space's conversation. Rust owns it; this follows the
 * Space and the tab in front, takes Rust's snapshot, and folds each event once,
 * in `seq` order. Anything it cannot fold in order sends it back to the
 * snapshot. The composer's own choices (the page chip, the selection, picked
 * elements) live here until they are sent.
 */
export class AssistSession {
  space = $state<string | null>(null);
  tab = $state<string | null>(null);
  status = $state.raw<PageAssistStatusV1 | null>(null);
  conversation = $state.raw<Conversation>(EMPTY);
  /** The first read of this Space has not come back yet. */
  loading = $state(false);
  /** Rust could not be reached. */
  unavailable = $state(false);
  /** The words each streaming text block received last, by `reply:block`. */
  arrivals = $state.raw<Readonly<Record<string, readonly Arrival[]>>>({});
  picks = $state.raw<Pick[]>([]);
  /** The tab an element is being picked on. */
  picking = $state<string | null>(null);
  /** The person took the page chip off for the next message. */
  pageOff = $state(false);
  /** Other tabs of the Space the person attached to the next message. */
  others = $state.raw<string[]>([]);
  selectionOn = $state(false);
  /** Why the last message or pick did not start. */
  notice = $state.raw<PageAssistFaultV1 | null>(null);
  sending = $state(false);
  styles = $state.raw<PageStylesV1 | null>(null);
  past = $state.raw<PageAssistPastV1[] | null>(null);

  private generation = 0;
  private queue: PageAssistChangedV1[] | null = null;
  private stopListening: (() => void) | null = null;
  private active = false;

  constructor(readonly profile: string) {}

  get latest() {
    return latestReply(this.conversation);
  }

  /** A reply is working or waiting on the person: Send becomes Stop. */
  get busy(): boolean {
    const latest = this.latest;
    return !!latest && open(latest);
  }

  /** The page in front can go along with a message. */
  get attachable(): boolean {
    return this.status?.availability === "ready" && !!this.status.page;
  }

  get pageAttached(): boolean {
    return this.attachable && !this.pageOff;
  }

  /** A message can be sent at all: a conversation exists and the panel can talk. */
  get talks(): boolean {
    const availability = this.status?.availability;
    return (
      !!this.conversation.id &&
      availability !== "ai_off" &&
      availability !== "private" &&
      availability !== "no_model"
    );
  }

  /** Listening starts before anything is read, so no event falls between. */
  async start() {
    if (this.active) return;
    this.active = true;
    const stop = await events.pageAssistChanged.listen(({ payload }) => this.receive(payload));
    if (!this.active) {
      stop();
      return;
    }
    this.stopListening = stop;
  }

  /** The Space and the tab in front. A new Space starts from its own snapshot. */
  follow(space: string | null, tab: string | null) {
    if (space !== this.space) {
      if (this.space) void quietly(commands.pageAssistClose(this.space), false);
      this.generation++;
      this.space = space;
      this.tab = tab;
      this.status = null;
      this.conversation = EMPTY;
      this.arrivals = {};
      this.picks = [];
      this.others = [];
      this.picking = null;
      this.styles = null;
      this.past = null;
      this.reset();
      this.queue = null;
      this.unavailable = false;
      this.loading = space !== null;
      if (space !== null) void this.read();
      return;
    }
    if (tab === this.tab) return;
    this.tab = tab;
    this.pageOff = false;
    this.selectionOn = false;
    this.styles = null;
    if (tab && this.others.includes(tab))
      this.others = this.others.filter((other) => other !== tab);
    void this.read();
  }

  /** Reads what is true now, without losing the conversation. */
  refresh() {
    if (this.space) void this.read();
  }

  private reset() {
    this.pageOff = false;
    this.selectionOn = false;
    this.notice = null;
  }

  private async read() {
    const space = this.space;
    const tab = this.tab;
    if (!space) return;
    const generation = ++this.generation;
    this.queue ??= [];
    let status: PageAssistStatusV1;
    try {
      status = await commands.pageAssistStatus(space, tab);
    } catch {
      if (generation !== this.generation) return;
      this.unavailable = true;
      this.loading = false;
      this.flush();
      return;
    }
    if (generation !== this.generation) return;
    this.unavailable = !status.conversation.id && status.availability === "unavailable";
    this.status = status;
    if (status.conversation.id !== this.conversation.id) {
      this.arrivals = {};
      this.picking = null;
    }
    this.conversation = status.conversation;
    if (!status.selection) this.selectionOn = false;
    this.loading = false;
    this.flush();
    if (status.styled && tab) void this.readStyles();
  }

  private flush() {
    const queued = this.queue ?? [];
    this.queue = null;
    for (const event of queued) this.receive(event);
  }

  private receive(event: PageAssistChangedV1) {
    if (event.space !== this.space) return;
    if (this.queue) {
      this.queue.push(event);
      return;
    }
    const before = this.conversation;
    const next = fold(before, event);
    if (next === "reload") {
      this.refresh();
      return;
    }
    if (!next) return;
    this.conversation = next;
    this.track(before, event);
    const kind = event.kind;
    if (
      kind.type === "block" &&
      kind.value.kind === "style" &&
      kind.value.style.state === "applied"
    )
      void this.readStyles();
  }

  /** Keeps the last few deltas of each streaming block, so their words can ink in. */
  private track(before: Conversation, event: PageAssistChangedV1) {
    const kind = event.kind;
    if (kind.type === "text") {
      const reply = before.messages.find((message) => message.id === event.reply);
      const block = reply?.kind === "reply" ? reply.blocks[kind.block] : undefined;
      const at = block?.kind === "text" ? block.text.length : 0;
      const key = `${event.reply}:${kind.block}`;
      const known = this.arrivals[key] ?? [];
      this.arrivals = {
        ...this.arrivals,
        [key]: [...known, { at, length: kind.delta.length }].slice(-FRESH),
      };
    } else if (kind.type === "done" || kind.type === "failed" || kind.type === "cancelled") {
      const prefix = `${event.reply}:`;
      if (Object.keys(this.arrivals).some((key) => key.startsWith(prefix)))
        this.arrivals = Object.fromEntries(
          Object.entries(this.arrivals).filter(([key]) => !key.startsWith(prefix)),
        );
    }
  }

  private attachments(): PageAssistAttachV1[] {
    const tab = this.tab;
    const out: PageAssistAttachV1[] = [];
    if (tab && this.pageAttached) out.push({ kind: "page", tab });
    for (const other of this.others) if (other !== tab) out.push({ kind: "page", tab: other });
    if (tab && this.selectionOn && this.status?.selection) out.push({ kind: "selection", tab });
    for (const pick of this.picks) out.push({ kind: "element", tab: pick.tab, pick: pick.pick });
    return out.slice(0, MAX_ATTACHMENTS);
  }

  /** True when Rust admitted the message; what it says arrives as events. */
  async ask(text: string, suggestion: PageAssistSuggestionV1 | null = null): Promise<boolean> {
    const words = text.trim().slice(0, MESSAGE_LIMIT);
    const space = this.space;
    if (!space || (!words && !suggestion) || this.busy || !this.talks || this.sending) return false;
    return this.send(space, words, suggestion, this.attachments(), true);
  }

  /** Sends the message a reply answered again, with the pages it had. */
  async retry(reply: number): Promise<boolean> {
    const space = this.space;
    const asked = askedBy(this.conversation, reply);
    if (!space || !asked || this.busy || this.sending) return false;
    const attachments = asked.attachments.flatMap<PageAssistAttachV1>((attachment) =>
      attachment.kind === "page"
        ? [{ kind: "page", tab: attachment.tab }]
        : attachment.kind === "selection"
          ? [{ kind: "selection", tab: attachment.tab }]
          : [],
    );
    return this.send(space, asked.text, asked.suggestion, attachments, false);
  }

  private async send(
    space: string,
    text: string,
    suggestion: PageAssistSuggestionV1 | null,
    attachments: PageAssistAttachV1[],
    composed: boolean,
  ): Promise<boolean> {
    this.sending = true;
    this.notice = null;
    try {
      const response = await commands.pageAssistAsk({
        space,
        conversation: this.conversation.id,
        text,
        suggestion,
        attachments,
      });
      if (response.reply === null) {
        this.notice = response.fault ?? { reason: "unavailable" };
        if (response.fault?.reason === "invalid") this.refresh();
        return false;
      }
      if (composed) {
        this.picks = [];
        this.others = [];
        this.pageOff = false;
        this.selectionOn = false;
      }
      return true;
    } catch {
      this.notice = { reason: "unavailable" };
      return false;
    } finally {
      this.sending = false;
    }
  }

  stop() {
    if (this.space) void quietly(commands.pageAssistStop(this.space), false);
  }

  /** Brings the cite's page forward when it is behind, then its words into view. */
  async cite(reply: number, cite: PageAssistCiteV1, active: string | null): Promise<boolean> {
    const id = this.conversation.id;
    if (!id) return false;
    if (cite.tab !== active) await quietly(commands.tabsActivate(cite.tab), null);
    return quietly(commands.pageAssistCite(id, reply, cite.n), false);
  }

  focus(reply: number, block: number, n: number | null) {
    const id = this.conversation.id;
    if (id) void quietly(commands.pageAssistFocus(id, reply, block, n), false);
  }

  async decide(reply: number, block: number, decision: PageAssistDecisionV1): Promise<boolean> {
    const id = this.conversation.id;
    return id ? quietly(commands.pageAssistDecide(id, reply, block, decision), false) : false;
  }

  resume(reply: number) {
    const id = this.conversation.id;
    if (id) void quietly(commands.pageAssistResume(id, reply), false);
  }

  /** The new Work's id once it started; the block itself follows by events. */
  async continueWork(reply: number, block: number): Promise<string | null> {
    const id = this.conversation.id;
    if (!id) return null;
    const result = await quietly(commands.pageAssistContinue(id, reply, block), {
      work: null,
      fault: { reason: "unavailable" } as const,
    });
    return result.work;
  }

  /** The person points at a part of the page; it joins the next message. */
  async pick(url: string | null): Promise<void> {
    const space = this.space;
    const tab = this.tab;
    if (!space || !tab || this.picking || !this.status?.can_pick) return;
    this.picking = tab;
    this.notice = null;
    const result = await quietly(commands.pageAssistPick(space, tab), {
      pick: null,
      fault: { reason: "unavailable" } as const,
    });
    if (this.picking !== tab || this.space !== space) return;
    this.picking = null;
    if (result.pick) {
      const pick = { ...result.pick, url };
      this.picks = [...this.picks.filter((known) => known.pick !== pick.pick), pick].slice(
        -(MAX_ATTACHMENTS - 2),
      );
    } else if (result.fault) this.notice = result.fault;
  }

  cancelPick() {
    const tab = this.picking;
    if (!tab) return;
    this.picking = null;
    void quietly(commands.pageAssistPickCancel(tab), false);
  }

  detach(pick: Pick) {
    this.picks = this.picks.filter((known) => known !== pick);
    void quietly(commands.pageAssistDetach(pick.tab, pick.pick), false);
  }

  /** Another of the Space's tabs goes along with the next message; the one in front is the page chip. */
  attachTab(tab: string) {
    if (tab === this.tab) this.pageOff = false;
    else if (!this.others.includes(tab)) this.others = [...this.others, tab].slice(-MAX_OTHERS);
  }

  detachTab(tab: string) {
    this.others = this.others.filter((other) => other !== tab);
  }

  /** Picks on a page that navigated or closed went with it, as did closed tabs. */
  forget(urlOf: (tab: string) => string | null | undefined) {
    const kept = this.picks.filter((pick) => urlOf(pick.tab) === pick.url);
    if (kept.length !== this.picks.length) this.picks = kept;
    const open = this.others.filter((tab) => urlOf(tab) !== undefined);
    if (open.length !== this.others.length) this.others = open;
  }

  async newConversation() {
    if (!this.space || this.conversation.messages.length === 0) return;
    if (await quietly(commands.pageAssistNew(this.space), false)) this.refresh();
  }

  async history() {
    if (!this.space) return;
    const view = await quietly(commands.pageAssistHistory(this.space), null);
    this.past = view?.conversations ?? [];
  }

  async openPast(id: string) {
    if (!this.space) return false;
    const opened = await quietly(commands.pageAssistOpen(this.space, id), false);
    if (opened) this.refresh();
    return opened;
  }

  async deletePast(id: string) {
    if (!this.space) return;
    const deleted = await quietly(commands.pageAssistDelete(this.space, id), false);
    if (!deleted) return;
    if (this.past) this.past = this.past.filter((entry) => entry.id !== id);
    if (id === this.conversation.id) this.refresh();
  }

  deleteCurrent() {
    if (this.conversation.messages.length) return this.deletePast(this.conversation.id);
  }

  async readStyles() {
    const tab = this.tab;
    if (!tab) return;
    const view = await quietly(commands.pageStyles(tab), null);
    if (view && tab === this.tab) this.styles = view;
  }

  async toggleStyle(id: string, enabled: boolean) {
    const tab = this.tab;
    if (!tab) return;
    const view = await quietly(commands.pageStyleToggle(tab, id, enabled), null);
    if (view && tab === this.tab) this.styles = view;
  }

  async removeStyle(id: string) {
    const tab = this.tab;
    if (!tab) return;
    const view = await quietly(commands.pageStyleRemove(tab, id), null);
    if (view && tab === this.tab) this.styles = view;
  }

  dispose() {
    this.cancelPick();
    if (this.space) void quietly(commands.pageAssistClose(this.space), false);
    this.active = false;
    this.generation++;
    this.stopListening?.();
    this.stopListening = null;
    this.queue = null;
    this.space = null;
    this.tab = null;
    this.conversation = EMPTY;
    this.status = null;
  }
}
