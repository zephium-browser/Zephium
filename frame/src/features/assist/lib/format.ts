import * as m from "$shared/i18n/messages";
import { getLocale } from "$shared/i18n/runtime";
import type {
  PageAssistCostV1,
  PageAssistStepReasonV1,
  WorkConfirmCategoryV1,
} from "$shared/ipc/bindings";

/** What a reply cost, as its footer's tooltip says it. */
export function costLine(cost: PageAssistCostV1): string {
  if (cost.kind === "local") return m.assist_cost_local({ model: cost.model });
  if (cost.kind === "byok")
    return cost.usd === null
      ? m.assist_cost_confirming({ model: cost.model })
      : m.assist_cost_own({ model: cost.model, amount: `$${cost.usd}`, provider: cost.provider });
  if (cost.credits === null) return m.assist_cost_confirming({ model: cost.model });
  return cost.credits === "1"
    ? m.assist_cost_credit({ model: cost.model })
    : m.assist_cost_credits({ model: cost.model, credits: cost.credits });
}

const QUOTED = /[“"]([^”"]*)[”"]/gu;

/** The control a step names, excluding a cross-site fill's value and source sites. */
export function controlOf(label: string): string {
  const destination = /^"(?:\\.|[^"\\])*" → ([^\n]*)\n/u.exec(label)?.[1] ?? label;
  return [...destination.matchAll(QUOTED)].findLast((match) => match[1])?.[1] ?? destination;
}

/** A step's words, with what it quotes (a control, a value) apart from the verb around it. */
export function labelParts(label: string): { text: string; named: boolean }[] {
  const parts: { text: string; named: boolean }[] = [];
  let at = 0;
  for (const match of label.matchAll(QUOTED)) {
    if (match.index > at) parts.push({ text: label.slice(at, match.index), named: false });
    if (match[1]) parts.push({ text: match[1], named: true });
    at = match.index + match[0].length;
  }
  if (at < label.length) parts.push({ text: label.slice(at), named: false });
  return parts;
}

/** Why a step failed, as the person can act on it. */
export function stepReason(reason: PageAssistStepReasonV1 | null): string {
  switch (reason) {
    case "refused":
      return m.assist_step_refused();
    case "unsuitable":
      return m.assist_step_unsuitable();
    case "page_changed":
      return m.assist_step_page_changed();
    case "no_effect":
      return m.assist_step_no_effect();
    default:
      return m.assist_step_failed();
  }
}

const plurals = new Map<string, Intl.PluralRules>();
/** One or many, as the language counts. */
export function isOne(count: number): boolean {
  const locale = getLocale();
  let rules = plurals.get(locale);
  if (!rules) plurals.set(locale, (rules = new Intl.PluralRules(locale)));
  return rules.select(count) === "one";
}

/** One quiet line on what confirming does, from Rust's category. */
export function consequence(category: WorkConfirmCategoryV1 | null, site: string | null): string {
  const args = { site: site ?? m.assist_this_page() };
  switch (category) {
    case "communication":
      return m.assist_consequence_communication(args);
    case "purchase":
      return m.assist_consequence_purchase(args);
    case "destructive":
      return m.assist_consequence_destructive(args);
    case "save":
      return m.assist_consequence_save(args);
    case "edit":
      return m.assist_consequence_edit(args);
    case "type":
      return m.assist_consequence_type(args);
    default:
      return m.assist_consequence_other(args);
  }
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/** When a past conversation last changed: "now", "5 min ago", "yesterday", a date. */
export function ago(then: number, now = Date.now(), locale?: string): string {
  const since = Math.max(0, now - then);
  if (since < MINUTE) return m.assist_history_now();
  const relative = new Intl.RelativeTimeFormat(locale, { numeric: "auto", style: "short" });
  if (since < HOUR) return relative.format(-Math.floor(since / MINUTE), "minute");
  if (since < DAY) return relative.format(-Math.floor(since / HOUR), "hour");
  if (since < 7 * DAY) return relative.format(-Math.floor(since / DAY), "day");
  return new Intl.DateTimeFormat(locale, { day: "numeric", month: "short" }).format(then);
}
