/// <reference types="node" />
import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import { getExtensionField, type AnyExtension } from "@tiptap/core";
import { noteSchemaExtensions } from "../lib/markdown/schema";
import { behaviour } from "../lib/editor/behaviour";

const registry = readFileSync(
  new URL("../../../../../crates/zephium-core/src/commands.rs", import.meta.url),
  "utf8",
);

/** A command's default keys on each platform, read from the native registry. */
function nativeKeys(id: string): string[] {
  const block = registry.match(new RegExp(`"${id.replace(".", "\\.")}",\\s*"[^"]*",\\s*([^\\n]+)`));
  if (!block) throw new Error(`${id} is not in the registry`);
  return [...block[1]!.matchAll(/"([^"]+)"/g)].map((key) => key[1]!);
}

const normal = (keys: string) =>
  keys
    .toLowerCase()
    .split(/[+-]/)
    .map((part) => (part === "cmdorctrl" || part === "cmd" || part === "ctrl" ? "mod" : part))
    .sort()
    .join("+");

function editorKeys(extensions: AnyExtension[]): string[] {
  return extensions.flatMap((extension) => {
    const shortcuts = getExtensionField<() => Record<string, unknown>>(
      extension,
      "addKeyboardShortcuts",
      { name: extension.name, options: extension.options, storage: {}, editor: null, type: null },
    );
    return shortcuts ? Object.keys(shortcuts()).map(normal) : [];
  });
}

test("Group into Folder leaves every note editor key to the editor", () => {
  const keys = nativeKeys("tab.group");
  expect(keys.length).toBeGreaterThan(0);
  const note = editorKeys([
    ...(noteSchemaExtensions() as AnyExtension[]),
    behaviour({ title: "", body: "", keydown: { current: null }, onlink: () => {} }),
  ]);
  // The registry was read and the editor's own keys were found.
  expect(note).toContain(normal("Mod-Shift-l"));
  for (const key of keys) expect(note).not.toContain(normal(key));
});
