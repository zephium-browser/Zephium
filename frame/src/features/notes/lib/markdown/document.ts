import type { JSONContent } from "@tiptap/core";
import type { Node } from "@tiptap/pm/model";
import { parse, type SourceBlock } from "./parse";
import { emittedList, writeBlock, type Previous } from "./serialize";

type Origin = SourceBlock & { index: number };

/** A note's Markdown as the editor holds it. Blocks the editor has not
 *  changed are written back exactly as they were read; ProseMirror keeps an
 *  unchanged block as the same node object, which is what `origins` keys on.
 *  Changed blocks are written fresh and cached by node the same way, so a
 *  keystroke re-serializes one block, not the note. */
export class MarkdownDocument {
  readonly json: JSONContent;
  #blocks: SourceBlock[];
  #lead: string;
  #tail: string;
  #origins = new WeakMap<Node, Origin>();
  #written = new WeakMap<Node, string>();

  constructor(markdown: string) {
    const parsed = parse(markdown);
    this.json = parsed.doc;
    this.#blocks = parsed.blocks;
    this.#lead = parsed.lead;
    const last = parsed.blocks.at(-1);
    // A note keeps whatever ending it had; a new one ends with a newline.
    this.#tail = last ? last.gap : "\n";
    if (last) last.gap = "";
  }

  /** Associates the editor's first document with the source it came from. */
  bind(doc: Node): void {
    if (doc.childCount !== this.#blocks.length) return;
    doc.forEach((child, _offset, index) => {
      this.#origins.set(child, { ...this.#blocks[index]!, index });
    });
  }

  /** The note as Markdown; with `blocks`, only its first few top-level
   *  blocks, which is all a title and a preview need. */
  serialize(doc: Node, blocks = Number.POSITIVE_INFINITY): string {
    let out = "";
    let previous: Previous = null;
    let previousOrigin: Origin | undefined;
    let count = 0;
    const end = Math.min(doc.childCount, blocks);
    for (let index = 0; index < end; index++) {
      const child = doc.child(index);
      let origin = this.#origins.get(child);
      let text = origin?.body ?? this.#written.get(child);
      // Two lists of one kind and marker that end up adjacent would merge
      // when read back; the second is rewritten with a different marker.
      const mine = text === undefined ? null : emittedList(child, text);
      const collides = !!mine && mine.type === previous?.type && mine.marker === previous.marker;
      if (text === undefined || collides) {
        origin = undefined;
        text = writeBlock(child, previous);
        if (!collides) this.#written.set(child, text);
      }
      previous = emittedList(child, text);
      if (text === "") {
        previousOrigin = undefined;
        continue;
      }
      if (count === 0) out += origin?.index === 0 ? this.#lead : "";
      else
        out +=
          origin && previousOrigin && origin.index === previousOrigin.index + 1
            ? previousOrigin.gap
            : "\n\n";
      out += text;
      previousOrigin = origin;
      count++;
    }
    return count ? out + this.#tail : "";
  }
}
