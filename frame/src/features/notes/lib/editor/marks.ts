import { shortcutHint } from "../platform";
import type { Editor } from "@tiptap/core";
import type { IconSvgElement } from "@hugeicons/svelte";
import CodeIcon from "@hugeicons/core-free-icons/CodeIcon";
import TextBoldIcon from "@hugeicons/core-free-icons/TextBoldIcon";
import TextItalicIcon from "@hugeicons/core-free-icons/TextItalicIcon";
import TextStrikethroughIcon from "@hugeicons/core-free-icons/TextStrikethroughIcon";
import * as m from "$shared/i18n/messages";

export type MarkId = "bold" | "italic" | "strike" | "code";

export type InlineStyle = {
  id: MarkId;
  label: () => string;
  icon: IconSvgElement;
  hint: string;
  toggle: (editor: Editor) => boolean;
};

export const MARKS: InlineStyle[] = [
  {
    id: "bold",
    label: m.note_bold,
    icon: TextBoldIcon,
    hint: shortcutHint("CmdOrCtrl+B"),
    toggle: (editor) => editor.chain().focus().toggleBold().run(),
  },
  {
    id: "italic",
    label: m.note_italic,
    icon: TextItalicIcon,
    hint: shortcutHint("CmdOrCtrl+I"),
    toggle: (editor) => editor.chain().focus().toggleItalic().run(),
  },
  {
    id: "strike",
    label: m.note_strike,
    icon: TextStrikethroughIcon,
    hint: shortcutHint("CmdOrCtrl+Shift+X"),
    toggle: (editor) => editor.chain().focus().toggleStrike().run(),
  },
  {
    id: "code",
    label: m.note_code,
    icon: CodeIcon,
    hint: shortcutHint("CmdOrCtrl+E"),
    toggle: (editor) => editor.chain().focus().toggleCode().run(),
  },
];

export function markActive(editor: Editor, id: MarkId): boolean {
  return editor.isActive(id);
}
