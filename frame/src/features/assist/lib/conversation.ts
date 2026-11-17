import type {
  PageAssistBlockV1,
  PageAssistChangedV1,
  PageAssistConversationV1,
  PageAssistEventV1,
  PageAssistMessageV1,
} from "$shared/ipc/bindings";

export type Conversation = PageAssistConversationV1;
export type Reply = Extract<PageAssistMessageV1, { kind: "reply" }>;
export type Person = Extract<PageAssistMessageV1, { kind: "person" }>;

export const EMPTY: Conversation = { id: "", title: null, messages: [], seq: 0 };

/** Rust keeps the same bound; a block past it is refused there too. */
const MAX_BLOCKS = 24;

/**
 * Folds one event into the snapshot, as Rust folded it before sending it.
 * `reload` when it cannot be folded in order: a gap in `seq`, a reply this
 * snapshot does not have, or the conversation changing shape. Null when it is
 * not this conversation's, or already folded.
 */
export function fold(
  view: Conversation,
  event: PageAssistChangedV1,
): Conversation | "reload" | null {
  if (event.kind.type === "invalidated") return "reload";
  if (event.conversation !== view.id || event.seq <= view.seq) return null;
  if (event.seq !== view.seq + 1) return "reload";
  const next = apply(view, event.reply, event.kind);
  return next ? { ...next, seq: event.seq } : "reload";
}

function apply(view: Conversation, reply: number, kind: PageAssistEventV1): Conversation | null {
  switch (kind.type) {
    case "message": {
      const message = kind.message;
      const at = view.messages.findIndex((known) => known.id === message.id);
      const messages = at === -1 ? [...view.messages, message] : view.messages.with(at, message);
      return { ...view, messages };
    }
    case "titled":
      return { ...view, title: kind.title };
    case "inert":
      return {
        ...view,
        messages: view.messages.map((message) =>
          message.kind === "reply" && message.id <= reply && !message.inert
            ? { ...message, inert: true }
            : message,
        ),
      };
    default: {
      const at = view.messages.findIndex(
        (message) => message.kind === "reply" && message.id === reply,
      );
      const found = view.messages[at];
      if (!found || found.kind !== "reply") return null;
      const changed = applyReply(found, kind);
      return changed ? { ...view, messages: view.messages.with(at, changed) } : null;
    }
  }
}

function applyReply(reply: Reply, kind: PageAssistEventV1): Reply | null {
  switch (kind.type) {
    case "phase":
      return { ...reply, phase: kind.phase };
    case "text": {
      const blocks = [...reply.blocks];
      if (kind.block === blocks.length && kind.block < MAX_BLOCKS)
        blocks.push({ kind: "text", text: "", cites: [] });
      const block = blocks[kind.block];
      if (block?.kind !== "text") return null;
      blocks[kind.block] = { ...block, text: block.text + kind.delta };
      return { ...reply, blocks };
    }
    case "cites": {
      const block = reply.blocks[kind.block];
      if (block?.kind !== "text") return null;
      return { ...reply, blocks: reply.blocks.with(kind.block, { ...block, cites: kind.cites }) };
    }
    case "block": {
      if (kind.value.kind === "text") return null;
      if (kind.block < reply.blocks.length)
        return { ...reply, blocks: reply.blocks.with(kind.block, kind.value) };
      if (kind.block === reply.blocks.length && kind.block < MAX_BLOCKS)
        return { ...reply, blocks: [...reply.blocks, kind.value] };
      return null;
    }
    case "state":
      return { ...reply, state: kind.state };
    case "done":
      return { ...reply, state: "done", phase: null, cost: kind.cost, end: kind.end };
    case "cost":
      return { ...reply, cost: kind.cost };
    case "failed":
      return { ...reply, state: "failed", phase: null, fault: kind.fault };
    case "cancelled":
      return { ...reply, state: "cancelled", phase: null };
    default:
      return null;
  }
}

/** A reply is still the conversation's: working, or waiting on the person. */
export const open = (reply: Reply) =>
  reply.state === "working" || reply.state === "waiting" || reply.state === "paused";

export function latestReply(view: Conversation): Reply | null {
  for (let at = view.messages.length - 1; at >= 0; at--) {
    const message = view.messages[at]!;
    if (message.kind === "reply") return message;
  }
  return null;
}

/** The person's message a reply answers. */
export function askedBy(view: Conversation, reply: number): Person | null {
  const at = view.messages.findIndex((message) => message.id === reply);
  for (let back = at - 1; back >= 0; back--) {
    const message = view.messages[back]!;
    if (message.kind === "person") return message;
  }
  return null;
}

/** The reply's words, cite marks taken out, for Copy. */
export function replyText(blocks: readonly PageAssistBlockV1[]): string {
  return blocks
    .flatMap((block) => (block.kind === "text" ? [block.text.replace(/⟦\d+⟧/gu, "")] : []))
    .join("\n\n")
    .trim();
}
