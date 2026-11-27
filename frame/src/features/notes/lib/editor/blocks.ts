import { shortcutHint } from "../platform";
import type { Editor } from "@tiptap/core";
import type { IconSvgElement } from "@hugeicons/svelte";
import { CheckListIcon, TextIcon } from "@hugeicons/core-free-icons";
import CodeIcon from "@hugeicons/core-free-icons/CodeIcon";
import DashedLine01Icon from "@hugeicons/core-free-icons/DashedLine01Icon";
import Heading01Icon from "@hugeicons/core-free-icons/Heading01Icon";
import Heading02Icon from "@hugeicons/core-free-icons/Heading02Icon";
import Heading03Icon from "@hugeicons/core-free-icons/Heading03Icon";
import LeftToRightListBulletIcon from "@hugeicons/core-free-icons/LeftToRightListBulletIcon";
import LeftToRightListNumberIcon from "@hugeicons/core-free-icons/LeftToRightListNumberIcon";
import QuoteDownIcon from "@hugeicons/core-free-icons/QuoteDownIcon";
import * as m from "$shared/i18n/messages";
import { toggleChecklist } from "./behaviour";

export type BlockId =
  | "title"
  | "heading"
  | "subheading"
  | "text"
  | "checklist"
  | "bullets"
  | "numbered"
  | "quote"
  | "code"
  | "divider";

export type Block = {
  id: BlockId;
  label: () => string;
  icon: IconSvgElement;
  hint?: string;
  /** Starts a new group in a menu. */
  separated?: boolean;
  /** Words a `/` command also answers to. */
  aliases: string[];
  /** Inserted rather than applied, so it is offered only as a command. */
  insert?: boolean;
};

export const BLOCKS: Block[] = [
  {
    id: "title",
    label: m.note_title_style,
    icon: Heading01Icon,
    hint: shortcutHint("CmdOrCtrl+Alt+1"),
    aliases: ["h1", "title"],
  },
  {
    id: "heading",
    label: m.note_heading,
    icon: Heading02Icon,
    hint: shortcutHint("CmdOrCtrl+Alt+2"),
    aliases: ["h2", "heading"],
  },
  {
    id: "subheading",
    label: m.note_subheading,
    icon: Heading03Icon,
    hint: shortcutHint("CmdOrCtrl+Alt+3"),
    aliases: ["h3", "subheading"],
  },
  {
    id: "text",
    label: m.note_text,
    icon: TextIcon,
    hint: shortcutHint("CmdOrCtrl+Alt+0"),
    aliases: ["text", "paragraph", "body"],
  },
  {
    id: "checklist",
    label: m.note_checklist,
    icon: CheckListIcon,
    hint: shortcutHint("CmdOrCtrl+Shift+L"),
    separated: true,
    aliases: ["todo", "task", "checkbox", "check"],
  },
  {
    id: "bullets",
    label: m.note_bullets,
    icon: LeftToRightListBulletIcon,
    hint: shortcutHint("CmdOrCtrl+Shift+8"),
    aliases: ["list", "ul", "bullet"],
  },
  {
    id: "numbered",
    label: m.note_numbered,
    icon: LeftToRightListNumberIcon,
    hint: shortcutHint("CmdOrCtrl+Shift+7"),
    aliases: ["ol", "number", "ordered"],
  },
  {
    id: "quote",
    label: m.note_quote,
    icon: QuoteDownIcon,
    hint: shortcutHint("CmdOrCtrl+Shift+B"),
    separated: true,
    aliases: ["blockquote", "citation"],
  },
  {
    id: "code",
    label: m.note_code_block,
    icon: CodeIcon,
    hint: shortcutHint("CmdOrCtrl+Alt+C"),
    aliases: ["code", "snippet", "pre"],
  },
  {
    id: "divider",
    label: m.note_divider,
    icon: DashedLine01Icon,
    aliases: ["hr", "rule", "line", "separator"],
    insert: true,
  },
];

export function activeBlock(editor: Editor): BlockId {
  if (editor.isActive("heading", { level: 1 })) return "title";
  if (editor.isActive("heading", { level: 2 })) return "heading";
  if (editor.isActive("heading")) return "subheading";
  if (editor.isActive("codeBlock")) return "code";
  if (
    editor.isActive("listItem", { checked: false }) ||
    editor.isActive("listItem", { checked: true })
  )
    return "checklist";
  if (editor.isActive("orderedList")) return "numbered";
  if (editor.isActive("bulletList")) return "bullets";
  if (editor.isActive("blockquote")) return "quote";
  return "text";
}

/** Makes the current block, or the selected ones, into `id`. Choosing what a
 *  block already is turns it back into text. */
export function applyBlock(editor: Editor, id: BlockId): boolean {
  const chain = () => editor.chain().focus();
  const current = activeBlock(editor);
  switch (id) {
    case "title":
      return chain().toggleHeading({ level: 1 }).run();
    case "heading":
      return chain().toggleHeading({ level: 2 }).run();
    case "subheading":
      return chain().toggleHeading({ level: 3 }).run();
    case "text":
      return current === "code" ? chain().toggleCodeBlock().run() : chain().setParagraph().run();
    case "checklist":
      return toggleChecklist(editor);
    case "bullets":
      if (current === "checklist") toggleChecklist(editor);
      return chain().toggleBulletList().run();
    case "numbered":
      if (current === "checklist") toggleChecklist(editor);
      return chain().toggleOrderedList().run();
    case "quote":
      return chain().toggleBlockquote().run();
    case "code":
      return chain().toggleCodeBlock().run();
    case "divider":
      return chain().setHorizontalRule().run();
  }
}

/** Blocks a `/` command offers for `query`, best match first. */
export function commandsFor(query: string): Block[] {
  const words = query.trim().toLowerCase();
  if (!words) return BLOCKS;
  const score = (block: Block) => {
    const names = [block.label().toLowerCase(), ...block.aliases];
    if (names.some((name) => name.startsWith(words))) return 2;
    if (names.some((name) => name.includes(words))) return 1;
    return 0;
  };
  return BLOCKS.map((block) => ({ block, score: score(block) }))
    .filter((entry) => entry.score > 0)
    .sort((a, b) => b.score - a.score)
    .map((entry) => entry.block);
}
