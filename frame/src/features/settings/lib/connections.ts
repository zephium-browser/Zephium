import type {
  WorkServerAuthV1,
  WorkServerDraftV1,
  WorkServerEnvV1,
  WorkServerRowV1,
  WorkServerSecretV1,
  WorkServerV1,
} from "$shared/ipc/bindings";

/** What the server editor holds; secret values live here only until Save. */
export type ServerForm = {
  name: string;
  kind: "stdio" | "http";
  /** The whole command line, as a person types it. */
  command: string;
  url: string;
  auth: WorkServerAuthV1;
  /** A pasted token; empty keeps the one the Keychain holds. */
  token: string;
  env: { name: string; value: string; secret: boolean; held: boolean }[];
};

/** Servers people add most, with the address their makers publish. */
export const PRESETS: { name: string; url: string; auth: WorkServerAuthV1 }[] = [
  { name: "Linear", url: "https://mcp.linear.app/mcp", auth: "oauth" },
  { name: "Notion", url: "https://mcp.notion.com/mcp", auth: "oauth" },
  { name: "Todoist", url: "https://ai.todoist.net/mcp", auth: "oauth" },
  { name: "Sentry", url: "https://mcp.sentry.dev/mcp", auth: "oauth" },
  { name: "Stripe", url: "https://mcp.stripe.com", auth: "oauth" },
  { name: "Figma", url: "http://127.0.0.1:3845/mcp", auth: "none" },
];

/** Names that hold a secret unless the person says otherwise. */
const SECRET = /TOKEN|KEY|SECRET|PASSWORD|PASS|CREDENTIAL|AUTH/u;

export function secretByName(name: string): boolean {
  return SECRET.test(name.toUpperCase());
}

/**
 * A command line split into words the way a shell would for the simple
 * cases people paste: spaces separate, quotes group, a backslash escapes.
 * Windows drive and UNC paths keep their separators.
 * `null` when a quote is left open.
 */
export function splitCommand(line: string): string[] | null {
  const words: string[] = [];
  let word = "";
  let started = false;
  let quote: string | null = null;
  let windowsPath = false;
  for (let index = 0; index < line.length; index++) {
    const c = line.charAt(index);
    if (!word) windowsPath ||= /^(?:[A-Za-z]:\\|\\\\)/u.test(line.slice(index));
    if (
      c === "\\" &&
      (!windowsPath || (!quote && line.charAt(index + 1) === "'")) &&
      quote !== "'" &&
      index + 1 < line.length
    ) {
      word += line.charAt(++index);
      started = true;
    } else if (quote) {
      if (c === quote) quote = null;
      else word += c;
    } else if (c === "'" || c === '"') {
      quote = c;
      started = true;
    } else if (/\s/u.test(c)) {
      if (started) words.push(word);
      word = "";
      started = false;
      windowsPath = false;
    } else {
      word += c;
      started = true;
    }
  }
  if (quote) return null;
  if (started) words.push(word);
  return words;
}

/** Joins words back into one line, quoting what needs it. */
export function joinCommand(words: readonly string[]): string {
  return words
    .map((word) => (/^[\w@%+=:,./-]+$/u.test(word) ? word : `'${word.replaceAll("'", "'\\''")}'`))
    .join(" ");
}

/** A short id for a server's tools: lowercase words joined by dashes. */
export function serverId(name: string, taken: readonly string[]): string {
  const base =
    name
      .normalize("NFKD")
      .toLowerCase()
      .replace(/[^a-z0-9]+/gu, "-")
      .replace(/^-+|-+$/gu, "")
      .slice(0, 32) || "server";
  let id = base;
  for (let n = 2; taken.includes(id); n++) id = `${base}-${n}`;
  return id;
}

export function emptyForm(): ServerForm {
  return {
    name: "",
    kind: "http",
    command: "",
    url: "",
    auth: "oauth",
    token: "",
    env: [],
  };
}

export function formOf(row: WorkServerRowV1): ServerForm {
  const { server } = row;
  const form = emptyForm();
  form.name = server.name;
  if (server.transport.kind === "stdio") {
    form.kind = "stdio";
    form.command = joinCommand([server.transport.command, ...server.transport.args]);
    form.env = server.transport.env.map((variable) => ({
      name: variable.name,
      value: variable.value ?? "",
      secret: variable.secret,
      held: variable.secret && row.secrets.includes(`env.${variable.name}`),
    }));
  } else {
    form.kind = "http";
    form.url = server.transport.url;
    form.auth = server.transport.auth;
  }
  return form;
}

export type FormFault = "name" | "command" | "url" | "env" | "token";

/** What keeps the form from saving, first field first. */
export function formFault(form: ServerForm, editing: WorkServerRowV1 | null): FormFault | null {
  if (!form.name.trim() || form.name.trim().length > 40) return "name";
  if (form.kind === "stdio") {
    const words = splitCommand(form.command.trim());
    if (!words?.length || words.some((word) => word.length > 1024) || words.length > 65)
      return "command";
    const names = form.env.map((variable) => variable.name.trim());
    if (
      names.some((name) => !/^[A-Z_][A-Z0-9_]{0,63}$/u.test(name)) ||
      new Set(names).size !== names.length ||
      form.env.some((variable) => variable.secret && !variable.value && !variable.held)
    )
      return "env";
    return null;
  }
  try {
    const url = new URL(form.url.trim());
    const local = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    if (url.protocol !== "https:" && !(url.protocol === "http:" && local)) return "url";
  } catch {
    return "url";
  }
  const held = editing?.secrets.includes("bearer") ?? false;
  if (form.auth === "bearer" && !form.token.trim() && !held) return "token";
  return null;
}

/** The draft Rust stores: the server without secrets, and the secrets beside it. */
export function draftOf(
  form: ServerForm,
  editing: WorkServerRowV1 | null,
  taken: readonly string[],
): WorkServerDraftV1 {
  const others = taken.filter((id) => id !== editing?.server.id);
  const keep = editing && editing.server.name === form.name.trim();
  const id = keep ? editing.server.id : serverId(form.name, others);
  const secrets: WorkServerSecretV1[] = [];
  let server: WorkServerV1;
  if (form.kind === "stdio") {
    const [command = "", ...args] = splitCommand(form.command.trim()) ?? [];
    const env: WorkServerEnvV1[] = form.env.map((variable) => {
      const name = variable.name.trim();
      if (variable.secret && variable.value)
        secrets.push({ account: `env.${name}`, value: variable.value });
      return {
        name,
        secret: variable.secret,
        value: variable.secret ? null : variable.value,
      };
    });
    server = {
      id,
      name: form.name.trim(),
      transport: { kind: "stdio", command, args, env },
      enabled: editing?.server.enabled ?? true,
    };
  } else {
    if (form.auth === "bearer" && form.token.trim())
      secrets.push({ account: "bearer", value: form.token.trim() });
    server = {
      id,
      name: form.name.trim(),
      transport: { kind: "http", url: form.url.trim(), auth: form.auth },
      enabled: editing?.server.enabled ?? true,
    };
  }
  return {
    server,
    secrets,
    previous: editing && editing.server.id !== id ? editing.server.id : null,
  };
}

/** Where a server lives, as its row shows it. */
export function serverAddress(server: WorkServerV1): string {
  if (server.transport.kind === "http") return server.transport.url.replace(/^https?:\/\//u, "");
  return joinCommand([server.transport.command, ...server.transport.args]);
}
