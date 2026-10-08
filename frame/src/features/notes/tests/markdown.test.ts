import { describe, expect, test } from "vitest";
import { getSchema, type JSONContent } from "@tiptap/core";
import type { Node } from "@tiptap/pm/model";
import { MarkdownDocument } from "../lib/markdown/document";
import { noteSchemaExtensions } from "../lib/markdown/schema";
import { parse } from "../lib/markdown/parse";
import {
  emittedList,
  expressible,
  semantic,
  writeBlock,
  type Previous,
} from "../lib/markdown/serialize";

const schema = getSchema(noteSchemaExtensions());

function load(markdown: string): { document: MarkdownDocument; doc: Node } {
  const document = new MarkdownDocument(markdown);
  const doc = schema.nodeFromJSON(document.json);
  document.bind(doc);
  return { document, doc };
}

/** The same content with one top-level block replaced, every other block
 *  kept as the identical node, as an editor transaction leaves them. */
function replaceBlock(doc: Node, index: number, json: JSONContent): Node {
  return doc.replace(
    doc.resolve(0).posAtIndex(index),
    doc.resolve(0).posAtIndex(index + 1),
    new (doc.slice(0).constructor as never as typeof import("@tiptap/pm/model").Slice)(
      schema.nodeFromJSON({ type: "doc", content: [json] }).content,
      0,
      0,
    ),
  );
}

/** Normalised content of a Markdown text, for comparing meaning. */
function meaning(markdown: string): JSONContent {
  return semantic(schema.nodeFromJSON(parse(markdown).doc).toJSON() as JSONContent);
}

/** Every block written from scratch, as if each had been edited. */
function rewritten(markdown: string): string {
  const doc = schema.nodeFromJSON(parse(markdown).doc);
  const parts: string[] = [];
  let previous: Previous = null;
  doc.forEach((child) => {
    const text = writeBlock(child, previous);
    if (text) parts.push(text);
    previous = emittedList(child, text);
  });
  return parts.join("\n\n") + "\n";
}

const CORPUS = [
  `# Trip planning

Some *emphasis* and _underscored_, **strong** and __strong__, \`code\`, ~~gone~~ and ~one~.
A [link](https://example.com "title") and <https://auto.link> and a bare https://bare.example.com.

- item one
- item two
  - nested
* star list

1. first
2. second

3) paren list

- [ ] todo
- [x] done
  - [ ] nested task

> quote line
> continues
>
> > nested quote

\`\`\`js
const a = 1;
\`\`\`

    indented code

Setext Heading
==============

---

| a | b |
|---|---|
| 1 | 2 |

<div>raw html</div>

Line with trailing hard break
next line\\
another

[[Wiki Link]] and [[Other|alias]] and footnote[^1].

[^1]: The note.
    continued

![image](pic.png) and [ref link][ref].

[ref]: https://example.com

Escapes: 1\\. not a list, \\*not em\\*, snake_case_word, a*b*c, 5 * 3, [draft], AT&amp;T, &unknown; x.
`,
  `---
tags: [a, b]
---
# With front matter

Body text.`,
  `\n\nLeading blank lines\n\n\n\nand extra gaps\n`,
  `* loose

* list

  with a second paragraph
`,
  `Plain note without a heading, no trailing newline`,
  `1. one
1. one again
1. and again

10. ten
11. eleven`,
  "````md\n```inner fence```\n````\n\n~~~\ntilde\n~~~\n",
  "Text with `` code ` tick `` and ` `` ` spans.\n",
  "Emoji 🙂 and CJK 漢字 and RTL שלום.\n",
];

describe("an untouched note", () => {
  test.each(CORPUS)("is written back byte for byte (%#)", (markdown) => {
    const { document, doc } = load(markdown);
    expect(document.serialize(doc)).toBe(markdown);
  });

  test("an empty note stays empty", () => {
    const { document, doc } = load("");
    expect(document.serialize(doc)).toBe("");
  });

  test("its first blocks read the same as the start of the whole", () => {
    const markdown = "# Title\n\nFirst paragraph.\n\n- one\n- two\n\nLast paragraph.\n";
    const { document, doc } = load(markdown);
    expect(document.serialize(doc, 2)).toBe("# Title\n\nFirst paragraph.\n");
    expect(document.serialize(doc, 99)).toBe(markdown);
  });
});

describe("a rewritten block", () => {
  test.each(CORPUS)("means what it meant (%#)", (markdown) => {
    expect(meaning(rewritten(markdown))).toEqual(meaning(markdown));
  });

  test("an edit changes only the edited block", () => {
    const markdown = CORPUS[0]!;
    const { document, doc } = load(markdown);
    const edited = replaceBlock(doc, 0, {
      type: "heading",
      attrs: { level: 1 },
      content: [{ type: "text", text: "Trip planning, revised" }],
    });
    expect(document.serialize(edited)).toBe(
      markdown.replace("# Trip planning\n", "# Trip planning, revised\n"),
    );
  });

  test("keeps the spelling it was read with", () => {
    const markdown = "* a\n* b\n\n3) c\n\n__bold__ and _italic_ and ~strike~\n";
    const { document, doc } = load(markdown);
    const touched = doc.content.content.reduce(
      (current, node, index) => replaceBlock(current, index, node.toJSON() as JSONContent),
      doc,
    );
    expect(document.serialize(touched)).toBe(markdown);
  });
});

describe("text that looks like syntax", () => {
  const cases: [string, string][] = [
    ["1. not a list", "1\\. not a list"],
    ["# not a heading", "\\# not a heading"],
    ["- not a bullet", "\\- not a bullet"],
    ["> not a quote", "\\> not a quote"],
    ["*not emphasis*", "\\*not emphasis\\*"],
    ["snake_case_word", "snake_case_word"],
    ["5 * 3 = 15", "5 * 3 = 15"],
    ["[draft] notes", "[draft] notes"],
    ["[text](not a link)", "\\[text\\](not a link)"],
    ["[[not wiki]]", "\\[[not wiki]]"],
    ["AT&T and &amp; literally", "AT&T and \\&amp; literally"],
    ["5 < 6 but <div> is not html", "5 < 6 but \\<div> is not html"],
    ["C:\\path\\to", "C:\\path\\to"],
    ["back\\*slash", "back\\\\\\*slash"],
    ["`tick`", "\\`tick\\`"],
    ["~~not struck~~", "\\~\\~not struck\\~\\~"],
    ["===", "\\==="],
  ];
  test.each(cases)("%s", (text, expected) => {
    const node = schema.nodeFromJSON({ type: "paragraph", content: [{ type: "text", text }] });
    const written = writeBlock(node);
    expect(written).toBe(expected);
    expect(meaning(written)).toEqual(
      semantic({
        type: "doc",
        content: [{ type: "paragraph", content: [{ type: "text", text }] }],
      }),
    );
  });
});

describe("structures the editor builds", () => {
  const cases: [string, JSONContent, string][] = [
    [
      "a checklist continues tight",
      {
        type: "bulletList",
        content: [
          {
            type: "listItem",
            attrs: { checked: false },
            content: [{ type: "paragraph", content: [{ type: "text", text: "milk" }] }],
          },
          {
            type: "listItem",
            attrs: { checked: true },
            content: [{ type: "paragraph", content: [{ type: "text", text: "bread" }] }],
          },
        ],
      },
      "- [ ] milk\n- [x] bread",
    ],
    [
      "an item with two paragraphs loosens its list",
      {
        type: "orderedList",
        attrs: { start: 1 },
        content: [
          {
            type: "listItem",
            content: [
              { type: "paragraph", content: [{ type: "text", text: "one" }] },
              { type: "paragraph", content: [{ type: "text", text: "more" }] },
            ],
          },
        ],
      },
      "1. one\n\n   more",
    ],
    [
      "marks nest and whitespace stays outside them",
      {
        type: "paragraph",
        content: [
          { type: "text", text: "a " },
          { type: "text", text: "bold ", marks: [{ type: "bold" }] },
          { type: "text", text: "both", marks: [{ type: "bold" }, { type: "italic" }] },
          { type: "text", text: " end" },
        ],
      },
      "a **bold *both*** end",
    ],
    [
      "links keep their destination and title",
      {
        type: "paragraph",
        content: [
          { type: "text", text: "see ", marks: [] },
          {
            type: "text",
            text: "the docs",
            marks: [
              { type: "link", attrs: { href: "https://example.com/a b", title: 'say "hi"' } },
            ],
          },
        ],
      },
      'see [the docs](<https://example.com/a b> "say \\"hi\\"")',
    ],
    [
      "code blocks outgrow the fences inside them",
      {
        type: "codeBlock",
        attrs: { language: "md" },
        content: [{ type: "text", text: "```\nx\n```" }],
      },
      "````md\n```\nx\n```\n````",
    ],
  ];
  test.each(cases)("%s", (_name, json, expected) => {
    const node = schema.nodeFromJSON(json);
    const written = writeBlock(node);
    expect(written).toBe(expected);
    expect(meaning(written)).toEqual(
      semantic(schema.nodeFromJSON({ type: "doc", content: [json] }).toJSON() as JSONContent),
    );
  });

  test("an empty task stays a task", () => {
    const { document } = load("");
    const doc = schema.nodeFromJSON({
      type: "doc",
      content: [
        {
          type: "bulletList",
          content: [
            { type: "listItem", attrs: { checked: false }, content: [{ type: "paragraph" }] },
            {
              type: "listItem",
              attrs: { checked: true },
              content: [{ type: "paragraph", content: [{ type: "text", text: "done" }] }],
            },
          ],
        },
      ],
    });
    const markdown = document.serialize(doc);
    expect(markdown).toBe("- [ ] \n- [x] done\n");
    expect(meaning(markdown)).toEqual(semantic(doc.toJSON() as JSONContent));
  });

  test("adjacent lists of one kind stay two lists", () => {
    const list = (text: string): JSONContent => ({
      type: "bulletList",
      content: [
        { type: "listItem", content: [{ type: "paragraph", content: [{ type: "text", text }] }] },
      ],
    });
    const { document } = load("");
    const doc = schema.nodeFromJSON({ type: "doc", content: [list("a"), list("b")] });
    const markdown = document.serialize(doc);
    expect(markdown).toBe("- a\n\n* b\n");
    expect(parse(markdown).doc.content).toHaveLength(2);
  });
});

/** Random documents inside the schema: whatever the editor can hold must
 *  survive being written and read back. */
describe("any document the editor can hold", () => {
  let seed = 7;
  const random = () => {
    seed = (seed * 1_103_515_245 + 12_345) % 2_147_483_648;
    return seed / 2_147_483_648;
  };
  const pick = <T>(values: T[]): T => values[Math.floor(random() * values.length)]!;
  const HOSTILE = [..."abc XYZ 123 *_`~[]()<>#!&\\|.-+=:;'\"é漢🙂"];
  const PROSE = [
    "note",
    "Lisbon",
    "a",
    "the",
    "1.",
    "e.g.",
    "C++",
    "snake_case",
    "don't",
    "(aside)",
    "[draft]",
    "5 * 3",
    "2024.",
    "AT&T",
    "#tag",
    "x_y",
    "—",
    "漢字",
    "🙂",
    '"quoted"',
    "end.",
  ];
  let alphabet = HOSTILE;
  const word = () =>
    alphabet === HOSTILE
      ? Array.from({ length: 1 + Math.floor(random() * 12) }, () => pick(HOSTILE)).join("")
      : Array.from({ length: 1 + Math.floor(random() * 4) }, () => pick(PROSE)).join(" ");
  const MARKS = [
    [],
    [],
    [{ type: "bold" }],
    [{ type: "italic" }],
    [{ type: "strike" }],
    [{ type: "code" }],
    [{ type: "bold" }, { type: "italic" }],
    [{ type: "link", attrs: { href: "https://example.com/x" } }],
  ];
  // Prose puts a space around most formatted words; hostile text never does.
  const inline = (): JSONContent[] =>
    Array.from({ length: 1 + Math.floor(random() * 4) }, () => ({
      type: "text",
      text: alphabet === PROSE && random() < 0.85 ? `${word()} ` : word(),
      marks: pick(MARKS),
    }));
  const paragraph = (): JSONContent => ({ type: "paragraph", content: inline() });
  const blockNode = (depth: number): JSONContent => {
    const kind = pick(
      depth > 1
        ? ["paragraph", "heading"]
        : ["paragraph", "heading", "bulletList", "orderedList", "blockquote", "codeBlock"],
    );
    switch (kind) {
      case "heading":
        return {
          type: "heading",
          attrs: { level: 1 + Math.floor(random() * 3) },
          content: inline(),
        };
      case "bulletList":
      case "orderedList":
        return {
          type: kind,
          content: Array.from({ length: 1 + Math.floor(random() * 3) }, () => ({
            type: "listItem",
            attrs: { checked: pick([null, true, false]) },
            content: [paragraph(), ...(random() < 0.3 ? [blockNode(depth + 1)] : [])],
          })),
        };
      case "blockquote":
        return { type: "blockquote", content: [blockNode(depth + 1)] };
      case "codeBlock":
        return {
          type: "codeBlock",
          attrs: { language: pick([null, "ts"]) },
          content: [{ type: "text", text: `${word()}\n${word()}` }],
        };
      default:
        return paragraph();
    }
  };

  /** Per character of each text block, the marks it carries. */
  const characters = (json: JSONContent): string[][] => {
    const blocks: string[][] = [];
    const walk = (node: JSONContent) => {
      if (node.type === "paragraph" || node.type === "heading") {
        const marks: string[] = [];
        for (const child of node.content ?? []) {
          const set = (child.marks ?? [])
            .map((mark) => JSON.stringify(mark))
            .sort()
            .join("|");
          for (const _ of child.text ?? "\u{fffc}") marks.push(set);
        }
        blocks.push(marks);
      }
      for (const child of node.content ?? [])
        if (node.type !== "paragraph" && node.type !== "heading") walk(child);
    };
    walk(json);
    return blocks;
  };
  /** A character that lost a mark `possible` still has, or gained one
   *  `expected` never had. */
  const formattingLost = (expected: JSONContent, possible: JSONContent, actual: JSONContent) => {
    const [full, least, got] = [characters(expected), characters(possible), characters(actual)];
    for (let block = 0; block < got.length; block++)
      for (let at = 0; at < got[block]!.length; at++) {
        const have = new Set(got[block]![at]!.split("|").filter(Boolean));
        const must = least[block]![at]!.split("|").filter(Boolean);
        const may = new Set(full[block]![at]!.split("|").filter(Boolean));
        if (must.some((mark) => !have.has(mark)) || [...have].some((mark) => !may.has(mark)))
          return { block, at, have: [...have], must, may: [...may] };
      }
    return null;
  };

  function roundTrips(runs: number) {
    const shape = (json: JSONContent): unknown => ({
      type: json.type,
      content: json.content?.filter((child) => child.type !== "text").map(shape),
    });
    const failures: string[] = [];
    let tried = 0;
    for (let run = 0; run < runs; run++) {
      const json: JSONContent = {
        type: "doc",
        content: Array.from({ length: 1 + Math.floor(random() * 5) }, () => blockNode(0)),
      };
      let doc: Node;
      try {
        doc = schema.nodeFromJSON(json);
        doc.check();
      } catch {
        continue;
      }
      tried++;
      const { document } = load("");
      const markdown = document.serialize(doc);
      const expected = semantic(doc.toJSON() as JSONContent);
      const actual = meaning(markdown);
      const reread = schema.nodeFromJSON(parse(markdown).doc);
      const words = (node: Node) => node.textContent.replace(/\s+/gu, "");
      expect(words(reread), JSON.stringify(markdown)).toEqual(words(doc));
      expect(shape(actual), JSON.stringify(markdown)).toEqual(shape(expected));
      // Formatting may only go missing where CommonMark cannot express it.
      const possible = semantic(expressible(doc).toJSON() as JSONContent);
      const lost = formattingLost(expected, possible, actual);
      if (lost) failures.push(JSON.stringify({ markdown, lost }, null, 1));
    }
    return { tried, failures };
  }

  test("hostile text keeps its words and shape", () => {
    alphabet = HOSTILE;
    roundTrips(500);
  });

  test("prose keeps all the formatting Markdown can express", () => {
    alphabet = PROSE;
    const { failures } = roundTrips(500);
    expect(failures).toEqual([]);
  });
});
