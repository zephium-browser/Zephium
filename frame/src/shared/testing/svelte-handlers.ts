/** Node-only behavioral probes of production callbacks, without mounting a browser. */
import { readFileSync } from "node:fs";
import { parse } from "svelte/compiler";
import ts from "typescript";

type Node = Record<string, unknown>;
function* nodes(value: unknown): Generator<Node> {
  if (!value || typeof value !== "object") return;
  if (Array.isArray(value)) {
    for (const entry of value) yield* nodes(entry);
    return;
  }
  const node = value as Node;
  yield node;
  for (const entry of Object.values(node)) yield* nodes(entry);
}

export function callbackSource(
  file: URL,
  kind: "event" | "function" | "effect" | "mount" | "destroy",
  name: string,
  contains = "",
): string | null {
  const source = readFileSync(file, "utf8");
  const ast = parse(source, { modern: true });
  for (const node of nodes(ast)) {
    let candidate: Node | undefined;
    if (kind === "event" && node.type === "Attribute" && node.name === name)
      candidate = [...nodes(node.value)].find((entry) => entry.type === "ArrowFunctionExpression");
    else if (
      kind === "function" &&
      node.type === "FunctionDeclaration" &&
      (node.id as Node)?.name === name
    )
      candidate = node;
    else if (
      (kind === "effect" || kind === "mount" || kind === "destroy") &&
      node.type === "CallExpression" &&
      (node.callee as Node)?.name ===
        (kind === "effect" ? "$effect" : kind === "mount" ? "onMount" : "onDestroy")
    ) {
      const argument = (node.arguments as Node[])[0];
      candidate =
        argument?.type === "Identifier"
          ? [...nodes(ast)].find(
              (entry) =>
                entry.type === "FunctionDeclaration" && (entry.id as Node)?.name === argument.name,
            )
          : [...nodes(node.arguments)].find((entry) => entry.type === "ArrowFunctionExpression");
    }
    if (!candidate || typeof candidate.start !== "number" || typeof candidate.end !== "number")
      continue;
    const body = source.slice(candidate.start, candidate.end);
    if (body.includes(contains)) return body;
  }
  return null;
}

export function callback<T>(source: string, context: Record<string, unknown>): T {
  const script = ts.transpileModule(`const selected = (${source});`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.ESNext },
  }).outputText;
  return new Function("scope", `with (scope) { ${script}\nreturn selected; }`)(context) as T;
}
