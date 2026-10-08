<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { Editor, type Node, type NodeViewRendererProps } from "@tiptap/core";
  import { Add01Icon, Note01Icon } from "@hugeicons/core-free-icons";
  import type { NoteReader, NoteSummary } from "$domain/notes";
  import * as m from "$shared/i18n/messages";
  import { MarkdownDocument } from "../../lib/markdown/document";
  import { noteSchemaExtensions } from "../../lib/markdown/schema";
  import { behaviour, triggerKey, type Trigger } from "../../lib/editor/behaviour";
  import { applyBlock, commandsFor } from "../../lib/editor/blocks";
  import { nodeViews, wikiTargets } from "../../lib/editor/views";
  import FormatBar from "./FormatBar.svelte";
  import Suggestions, { type Suggestion } from "./Suggestions.svelte";
  import "./editor.css";

  let {
    source,
    editable,
    density = "page",
    autofocus = false,
    linksRevision = 0,
    onchange,
    onleave,
    onopenlink,
    onopennote,
    resolve,
    find,
  }: {
    /** Markdown to load. Loaded once: a different note is a new editor. */
    source: string;
    editable: boolean;
    density?: "panel" | "page";
    /** Puts the caret in the title, for a note that has just been made. */
    autofocus?: boolean;
    linksRevision?: number;
    /** The text changed; `read` gives it as Markdown when it is wanted. */
    onchange: (read: NoteReader) => void;
    /** Focus left the note's text for somewhere outside the editor. */
    onleave?: () => void;
    onopenlink: (href: string) => void;
    onopennote: (target: string) => void;
    resolve: (targets: string[]) => Promise<Record<string, NoteSummary | null>>;
    find: (query: string) => Promise<NoteSummary[]>;
  } = $props();

  let host: HTMLDivElement;
  let editor = $state.raw<Editor | null>(null);
  let revision = $state(0);
  let focused = $state(false);
  let selecting = $state(false);
  let linking = $state(false);
  let trigger = $state.raw<Trigger | null>(null);
  let found = $state.raw<NoteSummary[]>([]);
  const keys: { current: ((event: KeyboardEvent) => boolean) | null } = { current: null };

  let showBar = $derived.by(() => {
    void revision;
    if (!editor || !editable || selecting || trigger) return false;
    const { selection } = editor.state;
    if (linking) return true;
    return focused && !selection.empty && !editor.isActive("codeBlock") && !("node" in selection);
  });

  /** Replaces the trigger's text with what was chosen. */
  function replaceTrigger(apply: () => void) {
    if (!editor || !trigger) return;
    editor.chain().focus().deleteRange({ from: trigger.from, to: trigger.to }).run();
    apply();
  }

  function linkTo(target: string) {
    replaceTrigger(() =>
      editor!
        .chain()
        .insertContent({ type: "wikiLink", attrs: { target, alias: null } })
        .insertContent(" ")
        .run(),
    );
  }

  let suggestions: Suggestion[] = $derived.by(() => {
    if (!trigger || !editor) return [];
    if (trigger.kind === "command")
      return [
        ...commandsFor(trigger.query).map((block) => ({
          id: block.id,
          label: block.label(),
          icon: block.icon,
          hint: block.hint,
          choose: () => replaceTrigger(() => applyBlock(editor!, block.id)),
        })),
        ...(!trigger.query || "link note".includes(trigger.query.toLowerCase())
          ? [
              {
                id: "link",
                label: m.note_link_search(),
                icon: Note01Icon,
                hint: "[[",
                choose: () => replaceTrigger(() => editor!.chain().insertContent("[[").run()),
              },
            ]
          : []),
      ];
    const query = trigger.query.trim();
    const items: Suggestion[] = found.map((note) => ({
      id: note.id,
      label: note.title,
      detail: note.preview || undefined,
      icon: Note01Icon,
      choose: () => linkTo(note.title),
    }));
    if (query && !found.some((note) => note.title.toLowerCase() === query.toLowerCase()))
      items.push({
        id: "create",
        label: m.note_link_create({ name: query }),
        icon: Add01Icon,
        choose: () => linkTo(query),
      });
    return items;
  });

  // Notes matching a `[[` query, a moment after typing stops.
  $effect(() => {
    if (trigger?.kind !== "link") return;
    const query = trigger.query;
    let live = true;
    const timer = setTimeout(() => {
      void find(query).then((notes) => {
        if (live) found = notes;
      });
    }, 80);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  });

  /** Marks every `[[link]]` as leading somewhere or not yet. */
  let resolving = 0;
  function markLinks() {
    if (!editor) return;
    const targets = wikiTargets(editor.state.doc);
    if (!targets.length) return;
    const request = ++resolving;
    void resolve(targets).then((notes) => {
      if (request !== resolving) return;
      for (const element of host.querySelectorAll<HTMLElement>("[data-wiki-link]")) {
        const note = notes[element.dataset.wikiLink ?? ""];
        element.dataset.state = note ? "resolved" : "missing";
        element.title = note
          ? note.title
          : m.note_link_missing({ name: element.dataset.wikiLink ?? "" });
      }
    });
  }

  let linkTimer: ReturnType<typeof setTimeout> | undefined;
  $effect(() => {
    void linksRevision;
    untrack(markLinks);
  });

  onMount(() => {
    const document = new MarkdownDocument(source);
    const views = nodeViews({ check: m.note_check(), kept: m.note_markdown_kept() }, onopennote);
    const withViews = noteSchemaExtensions().map((extension) => {
      const view = views[extension.name];
      return view && extension.type === "node"
        ? (extension as Node).extend({
            addNodeView: () => (props: NodeViewRendererProps) =>
              view(props.node, props.view, props.getPos, props.decorations, props.innerDecorations),
          })
        : extension;
    });
    const instance = new Editor({
      element: host,
      injectCSS: false,
      editable,
      extensions: [
        ...withViews,
        behaviour({
          title: m.note_title_placeholder(),
          body: m.note_body_placeholder(),
          keydown: keys,
          onlink: () => (linking = true),
        }),
      ],
      content: document.json,
      editorProps: {
        attributes: {
          class: "note-document",
          role: "textbox",
          "aria-multiline": "true",
          "aria-label": m.note_document(),
          spellcheck: "true",
          autocapitalize: "sentences",
        },
        handleClick(_view, _pos, event) {
          const anchor = (event.target as HTMLElement).closest<HTMLAnchorElement>("a[href]");
          if (!anchor) return false;
          event.preventDefault();
          onopenlink(anchor.getAttribute("href") ?? "");
          return true;
        },
      },
      onUpdate: ({ editor: current }) => {
        // A document never changes once made, so reading it later, after
        // more typing or after this editor is gone, still gives this text.
        const doc = current.state.doc;
        onchange((blocks) => document.serialize(doc, blocks));
        clearTimeout(linkTimer);
        // Only a link drawn since the last pass has no state yet; without
        // one there is nothing to look up.
        linkTimer = setTimeout(() => {
          if (host.querySelector("[data-wiki-link]:not([data-state])")) markLinks();
        }, 300);
      },
      onTransaction: ({ editor: current }) => {
        trigger = triggerKey.getState(current.state)?.trigger ?? null;
        revision++;
      },
      onFocus: () => (focused = true),
      onBlur: ({ event }) => {
        // Focus moving into the bar (its link field) keeps the bar.
        if (!(event.relatedTarget as HTMLElement | null)?.closest(".note-format, .note-float")) {
          focused = false;
          linking = false;
          onleave?.();
        }
      },
    });
    document.bind(instance.state.doc);
    // A note with nothing in it starts at its title.
    const doc = instance.state.doc;
    if (
      editable &&
      doc.childCount === 1 &&
      doc.firstChild?.type.name === "paragraph" &&
      !doc.firstChild.content.size
    )
      instance.commands.setContent(
        { type: "doc", content: [{ type: "heading", attrs: { level: 1 } }] },
        { emitUpdate: false },
      );
    if (autofocus) instance.commands.focus("end");
    editor = instance;
    markLinks();
    // The bar waits for a drag-selection to finish rather than chase it.
    const press = (event: PointerEvent) => {
      if (event.button === 0) selecting = true;
    };
    const release = () => (selecting = false);
    host.addEventListener("pointerdown", press);
    window.addEventListener("pointerup", release);
    return () => {
      host.removeEventListener("pointerdown", press);
      window.removeEventListener("pointerup", release);
      clearTimeout(linkTimer);
      instance.destroy();
      editor = null;
    };
  });

  $effect(() => {
    editor?.setEditable(editable, false);
  });

  export function focus() {
    editor?.commands.focus();
  }
</script>

<div class="note-editor" data-density={density} data-editable={editable} bind:this={host}></div>
{#if editor && showBar}<FormatBar {editor} {revision} bind:linking />{/if}
{#if editor && trigger}<Suggestions
    {editor}
    at={trigger.from}
    items={suggestions}
    label={trigger.kind === "link" ? m.note_link_search() : m.note_formatting()}
    controller={keys}
  />{/if}
