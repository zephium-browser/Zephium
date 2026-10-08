import type {
  WorkConfirmCategoryV1,
  WorkExecutionFact,
  WorkHumanPageV1,
  WorkPageV1,
  WorkStepFact,
} from "$shared/ipc/bindings";
import { pageFrameUrl } from "$domain/resources";
import { vendorHost } from "../../lib/vendors";
import { registrableSite, siteName } from "../../lib/run/site";
import type { RunTurn } from "../../lib/project-environment-thread";

export { registrableSite, siteName } from "../../lib/run/site";

/** Where a decided ask stands; `open` is the only state that takes a decision. */
type AskState = "open" | "working" | "done" | "declined" | "failed" | "unknown" | "gone";

type Base = {
  /** The step the decision goes to. */
  step: string;
  /** The lead part it belongs to, for its row on the canvas. */
  part: string | null;
  state: AskState;
};

export type ConfirmAsk = Base & {
  kind: "confirm";
  site: string;
  category: WorkConfirmCategoryV1;
  headline: string;
  /** The page's own control, as the button reads: "Send", "Request to book". */
  verb: string;
  /** What Rust will press, in its words: "press Send". */
  action: string;
  text: string | null;
  facts: readonly { label: string; value: string }[];
  /** The page as it stands, captured when it was held. */
  frame: string | null;
  url: string | null;
  provenance: readonly string[];
  runOption: boolean;
  allowedForRun: boolean;
  note: string | null;
};

export type EntryAsk = Base & {
  kind: "entry";
  /** What people call the site: "Slack". */
  name: string;
  host: string | null;
  /** Every service one question covers: "Slack, Gmail and Calendar" is three. */
  hosts: string[];
  /** The agent's plan, in its words; labelled as such, never as a fact. */
  plan: string;
  always: string;
  answer: string | null;
};

export type ContextSource = "history" | "notes" | "tabs";

export type ContextAsk = Base & {
  kind: "context";
  source: ContextSource;
  /** Why the agent wants it, in its words. */
  reason: string;
  answer: string | null;
};

export type ConnectionAsk = Base & {
  kind: "connection";
  /** "GitHub". */
  service: string;
  /** "gh", when a command line tool is the route. */
  tool: string | null;
  host: string | null;
  use: string;
  web: string;
  answer: string | null;
};

export type QuestionAsk = Base & {
  kind: "question";
  prompt: string;
  options: readonly string[];
  answer: string | null;
};

/** "Read Lunios?": a folder the request named, read for this work once allowed. */
export type FolderAsk = Base & {
  kind: "folder";
  /** The folder's own name: "Lunios". */
  name: string;
  path: string;
  /** Rust's option words, as they go back: allow first, then not now. */
  allow: string;
  decline: string;
  /** A folder the run asks for as it works: chosen in the system's panel, the answer is its path. */
  choose: boolean;
  /** Why the run needs it, in the agent's words. */
  reason: string | null;
  answer: string | null;
};

/** "Open this address?": one the agent wrote itself after reading the person's own data. */
export type AddressAsk = Base & {
  kind: "address";
  url: string;
  /** The host the address goes to, the part worth reading first. */
  host: string;
  /** Rust's option words, as they go back: open, allow the site for the request, decline. */
  open: string;
  allowSite: string;
  decline: string;
  answer: string | null;
};

export type SignInAsk = {
  kind: "sign_in";
  step: string;
  part: string | null;
  host: string;
  /** The held page, for Open and "I've signed in". */
  page: WorkHumanPageV1;
  state: "open" | "working";
};

export type Ask =
  | ConfirmAsk
  | EntryAsk
  | ContextAsk
  | ConnectionAsk
  | FolderAsk
  | AddressAsk
  | QuestionAsk
  | SignInAsk;

/** Rust's fixed words for the questions it puts itself (`work_sites.rs`, `work_context_tools.rs`). */
export const ASK_WORDS = {
  allow: "Allow",
  notNow: "Not now",
  alwaysFor: "Always for ",
  web: "Use the website instead",
  chooseFolder: "Choose folder…",
} as const;

const CONTEXT_PROMPTS: Record<ContextSource, string> = {
  history: "Use your history?",
  notes: "Use your notes?",
  tabs: "Use your open tabs?",
};

function hostOf(url: string): string | null {
  try {
    return new URL(url).hostname.replace(/^www\./u, "");
  } catch {
    return null;
  }
}

const stateOf = (step: WorkStepFact, answered: boolean): AskState => {
  if (step.status === "running") return answered ? "working" : "open";
  if (step.status === "succeeded") return "done";
  if (step.status === "cancelled") return answered ? "declined" : "gone";
  if (step.status === "outcome_unknown") return "unknown";
  return answered ? "failed" : "gone";
};

/** The button word: the page's own control when it is a short name, else the kind's verb. */
export function confirmVerb(category: WorkConfirmCategoryV1, action: string): string {
  const pressed = /^press (.+)$/u.exec(action.trim())?.[1]?.trim() ?? "";
  if (
    pressed &&
    !/^Enter\b/u.test(pressed) &&
    pressed.length <= 24 &&
    pressed.split(/\s+/u).length <= 3
  )
    return pressed;
  switch (category) {
    case "communication":
      return "Send";
    case "purchase":
      return "Book";
    case "destructive":
      return "Delete";
    case "save":
    case "edit":
      return "Save";
    case "type":
      return "Type";
  }
}

/** The running page task a site's entry question is about. */
function entryHost(steps: readonly WorkStepFact[], name: string): string | null {
  const tasks = steps.flatMap((step) =>
    step.kind.kind === "read" && step.kind.goal && step.status === "running"
      ? [hostOf(step.kind.url)].filter((host): host is string => !!host)
      : [],
  );
  const word = name.toLocaleLowerCase().replace(/\s+/gu, "");
  const known = vendorHost(undefined, name);
  const called = (host: string) =>
    siteName(host).toLocaleLowerCase().replace(/\s+/gu, "").endsWith(word);
  return (
    tasks.find(
      (host) =>
        (known && (host === known || host.endsWith(`.${known}`))) ||
        host === word ||
        host.split(".").includes(word.replace(/\.[a-z]+$/u, "")) ||
        called(host),
    ) ?? (tasks.length === 1 ? tasks[0]! : known)
  );
}

type AskBase = Pick<Base, "step" | "part" | "state">;
type Words = { prompt: string; options: readonly string[]; answer: string | null };

/** "Work in your Slack?": a site's entry question, named by its "Always for" option or its words. */
function entryOf(
  base: AskBase,
  { prompt, options, answer }: Words,
  steps: readonly WorkStepFact[],
): EntryAsk | null {
  const always = options.find((option) => option.startsWith(ASK_WORDS.alwaysFor));
  const name =
    always?.slice(ASK_WORDS.alwaysFor.length) ?? /^Work in your (.+?)\?/u.exec(prompt)?.[1];
  if (!name) return null;
  const lead = `Work in your ${name}?`;
  const plan = prompt.startsWith(lead) ? prompt.slice(lead.length).trim() : prompt;
  const hosts = name.split(/, | and /u).flatMap((service) => entryHost(steps, service) ?? []);
  return {
    ...base,
    kind: "entry",
    name,
    host: hosts.length > 1 ? null : entryHost(steps, name),
    hosts: [...new Set(hosts)],
    plan: plan.replace(/\.$/u, ""),
    always: always ?? `${ASK_WORDS.alwaysFor}${name}`,
    answer,
  };
}

/** "Use your history?": which of the person's things, by its fixed words or, failing them, its noun. */
function contextOf(base: AskBase, { prompt, answer }: Words, loose: boolean): ContextAsk | null {
  for (const source of Object.keys(CONTEXT_PROMPTS) as ContextSource[]) {
    const lead = CONTEXT_PROMPTS[source];
    if (prompt.startsWith(lead))
      return { ...base, kind: "context", source, reason: prompt.slice(lead.length).trim(), answer };
  }
  if (!loose) return null;
  const source = /\b(history|notes|tabs)\b/iu.exec(prompt)?.[1]?.toLocaleLowerCase() as
    ContextSource | undefined;
  return source ? { ...base, kind: "context", source, reason: prompt, answer } : null;
}

/** "Use GitHub (gh)? …": a connected service or its tool, instead of the website. */
function connectionOf(base: AskBase, { prompt, options, answer }: Words): ConnectionAsk | null {
  const service = /^Use (.+?)(?: \(([a-z0-9-]+)\))?\?(?:\s+(.*))?$/su.exec(prompt);
  const [first, second] = options;
  if (!service || !first) return null;
  const name = service[1]!;
  return {
    ...base,
    kind: "connection",
    service: name,
    tool: service[2] ?? null,
    host: vendorHost(undefined, name),
    use: first,
    web: second ?? ASK_WORDS.web,
    answer,
  };
}

/** The prompt is the address itself; anything else is a plain question. */
function addressOf(base: AskBase, { prompt, options, answer }: Words): AddressAsk | null {
  const [open, allowSite, decline] = options;
  if (!open || !allowSite || !decline) return null;
  let host: string;
  try {
    const url = new URL(prompt);
    if (url.protocol !== "https:") return null;
    host = url.host;
  } catch {
    return null;
  }
  return { ...base, kind: "address", url: prompt, host, open, allowSite, decline, answer };
}

/** "Read Lunios?": the folder rides the step's local fact; its name is the path's last. */
function folderOf(
  base: AskBase,
  step: WorkStepFact,
  { prompt, options, answer }: Words,
): FolderAsk | null {
  const path = step.local?.folder?.trim();
  if (!path) return null;
  const name = folderName(path);
  const choose = options[0] === ASK_WORDS.chooseFolder;
  const reason = choose ? prompt.replace(/^[^?]*\?\s*/u, "").trim() : "";
  return {
    ...base,
    kind: "folder",
    name,
    path,
    allow: options[0] ?? ASK_WORDS.allow,
    decline: options[1] ?? ASK_WORDS.notNow,
    choose,
    reason: reason || null,
    answer,
  };
}

/** A folder's own name, the last part of its path: "Documents". */
export const folderName = (path: string) => path.replace(/\/+$/u, "").split("/").at(-1) || path;

function fromAsk(
  step: WorkStepFact,
  steps: readonly WorkStepFact[],
): EntryAsk | ContextAsk | ConnectionAsk | FolderAsk | AddressAsk | QuestionAsk | null {
  if (step.kind.kind !== "ask") return null;
  const { prompt, options, purpose } = step.kind;
  const answer = step.kind.answer ?? null;
  const base = { step: step.id, part: step.part ?? null, state: stateOf(step, !!answer) };
  const words = { prompt, options, answer };
  const question: QuestionAsk = { ...base, kind: "question", prompt, options, answer };
  // The runtime says what it asks; the words decide only for runs from before it did.
  switch (purpose) {
    case "entry":
      return entryOf(base, words, steps) ?? question;
    case "context":
      return contextOf(base, words, true) ?? question;
    case "connection":
      return connectionOf(base, words) ?? question;
    case "folder":
      return folderOf(base, step, words) ?? question;
    case "address":
      return addressOf(base, words) ?? question;
    case "question":
    case "budget":
    case "confirm":
      return question;
  }
  const [first, second, third] = options;
  if (
    options.length === 3 &&
    first === ASK_WORDS.allow &&
    third === ASK_WORDS.notNow &&
    second?.startsWith(ASK_WORDS.alwaysFor)
  )
    return entryOf(base, words, steps);
  if (options.length === 2 && first === ASK_WORDS.allow && second === ASK_WORDS.notNow) {
    const context = contextOf(base, words, false);
    if (context) return context;
  }
  if (options.length === 2 && second === ASK_WORDS.web) {
    const connection = connectionOf(base, words);
    if (connection) return connection;
  }
  return question;
}

function fromConfirm(step: WorkStepFact, pages: readonly WorkPageV1[]): ConfirmAsk | null {
  if (step.kind.kind !== "confirm") return null;
  const held = step.kind.confirm;
  const decision = held.decision ?? null;
  const page = held.page ? pages.find((entry) => entry.step === held.page) : undefined;
  const frame = page?.frame ? pageFrameUrl(page.attempt, page.step, page.frame.generation) : null;
  const state: AskState =
    decision === "declined"
      ? step.status === "running"
        ? "working"
        : "declined"
      : stateOf(step, decision !== null);
  return {
    kind: "confirm",
    step: step.id,
    part: step.part ?? null,
    state,
    site: held.site,
    category: held.category,
    headline: held.headline,
    verb: confirmVerb(held.category, held.action),
    action: held.action,
    text: held.text ?? null,
    facts: held.facts ?? [],
    frame,
    url: page?.url ?? null,
    provenance: held.provenance ?? [],
    runOption: !!held.run_option,
    allowedForRun: decision === "allowed_for_run",
    note: step.note ?? null,
  };
}

/**
 * Every question a run put to the person, in the order it asked them, open and
 * decided alike: a decided ask stays as a quiet receipt where it stood.
 */
export function asksOf(
  execution: WorkExecutionFact,
  pages: readonly WorkPageV1[] = [],
  held: readonly WorkHumanPageV1[] = [],
): Ask[] {
  const steps = execution.steps ?? [];
  const asks: Ask[] = [];
  for (const step of steps) {
    const ask = fromConfirm(step, pages) ?? fromAsk(step, steps);
    if (ask) asks.push(ask);
  }
  for (const page of held) {
    if (page.reason !== "sign_in" || page.phase === "reading" || page.phase === "released")
      continue;
    const step = steps.find((entry) => entry.id === page.id.step);
    const url = step?.kind.kind === "read" ? step.kind.url : null;
    const host = url ? hostOf(url) : null;
    if (!host) continue;
    asks.push({
      kind: "sign_in",
      step: page.id.step,
      part: step?.part ?? null,
      host,
      page,
      state: page.phase === "continuing" ? "working" : "open",
    });
  }
  return asks;
}

/** The asks still waiting on the person; the newest leads. */
export const openAsks = (asks: readonly Ask[]) =>
  asks.filter((ask) => ask.state === "open").reverse();

/** Where an ask stands on the canvas: its lead part, else the part of its site. */
export function askPart(ask: Ask): string | null {
  if (ask.part) return ask.part;
  if (ask.kind === "confirm") return registrableSite(ask.site);
  if (ask.kind === "sign_in") return registrableSite(ask.host);
  if (ask.kind === "entry" && ask.host) return registrableSite(ask.host);
  if (ask.kind === "connection" && ask.host) return registrableSite(ask.host);
  return null;
}

/** The asks that belong on one part's row. */
export const asksFor = (asks: readonly Ask[], part: string) =>
  asks.filter((ask) => askPart(ask) === part);

/** The questions runs put to the person and the answers, and words sent in while they ran, in order. */
export function exchangeOf(runs: readonly WorkExecutionFact[]): RunTurn[] {
  const turns: RunTurn[] = [];
  for (const run of runs) {
    const questions = new Map(
      asksOf(run).flatMap((ask) => (ask.kind === "question" ? [[ask.step, ask] as const] : [])),
    );
    for (const step of run.steps ?? []) {
      if (step.kind.kind === "steer" && step.kind.text.trim())
        turns.push({ kind: "steer", text: step.kind.text.trim() });
      const ask = questions.get(step.id);
      if (ask && ask.state !== "gone")
        turns.push({ kind: "ask", question: ask.prompt, answer: ask.answer?.trim() || null });
    }
  }
  return turns;
}
