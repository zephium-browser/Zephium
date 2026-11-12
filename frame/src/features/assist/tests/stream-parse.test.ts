import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { compile, parse as component } from "svelte/compiler";
import { expect, test } from "vitest";
import { parse, type Arrival } from "../lib/markdown";

test("one streaming block does not reparse finished replies", async () => {
  const source = readFileSync(
    new URL("../components/blocks/TextBlock.svelte", import.meta.url),
    "utf8",
  );
  const script = component(source, { modern: true }).instance!.content.body;
  const derivations = script
    .filter(
      (node) =>
        node.type === "VariableDeclaration" &&
        node.declarations.some(
          (declaration) =>
            declaration.id.type === "Identifier" &&
            ["incoming", "tree"].includes(declaration.id.name),
        ),
    )
    .map((node) => {
      if (
        !("start" in node) ||
        !("end" in node) ||
        typeof node.start !== "number" ||
        typeof node.end !== "number"
      )
        throw new Error("missing production derivation source span");
      return source.slice(node.start, node.end);
    })
    .join("\n");
  // Compile the production markdown derivations and prop boundary without a DOM.
  const code = compile(
    `<script>let {text, arrivals = [], parse} = $props();\n${derivations}\nexport function readTree() { return tree; }</script>`,
    { generate: "client", dev: false },
  ).js.code.replace(
    /(['"])svelte\/internal\/([^'"]+)\1/gu,
    (_whole, _quote: string, path: string) =>
      JSON.stringify(import.meta.resolve(`svelte/internal/${path}`)),
  );
  const runtime = createRequire(import.meta.url)("svelte/internal/client");
  const { default: Block } = await import(
    /* @vite-ignore */ `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`
  );
  const arrivals = runtime.state({} as Record<string, readonly Arrival[]>);
  const current = runtime.state("Streaming");
  const counts = [0, 0, 0];
  const stop = runtime.effect_root(() => {
    for (let index = 0; index < 3; index++) {
      const block = Block(
        {},
        {
          get text() {
            return index === 2 ? runtime.get(current) : `Finished **reply ${index}**`;
          },
          get arrivals() {
            return runtime.get(arrivals)[`${index}:0`];
          },
          parse(text: string, fresh: readonly Arrival[]) {
            counts[index]!++;
            return parse(text, fresh);
          },
        },
      );
      runtime.render_effect(() => block.readTree());
    }
  });
  try {
    for (let delta = 0; delta < 3; delta++) {
      runtime.set(current, `Streaming ${delta}`);
      runtime.set(arrivals, { "2:0": [{ at: 0, length: 5 }] });
      runtime.flush();
    }
    expect(counts).toEqual([1, 1, 4]);
  } finally {
    stop();
  }
});
