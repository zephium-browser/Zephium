import * as m from "$shared/i18n/messages";
import type { PageAssistAvailabilityV1, PageAssistFaultV1 } from "$shared/ipc/bindings";

type FaultFix = { label: string; command: string };

export type FaultView = {
  /** Seconds Rust asked to wait, when it said. */
  wait?: number | null;
  message: string;
  /** Where the person fixes it, when they can. */
  fix: FaultFix | null;
  /** The same message may be sent again as is; a countdown waits first. */
  retry: "no" | "now" | "wait";
};

/** When Zephium AI's daily limit next resets: 00:00 UTC, on the person's own clock. */
function dailyResetTime(now: Date = new Date()): string {
  const next = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() + 1);
  return new Intl.DateTimeFormat(undefined, { hour: "numeric", minute: "2-digit" }).format(next);
}

const account = (label: string): FaultFix => ({ label, command: "settings.account" });
const models = (label: string): FaultFix => ({ label, command: "settings.ai" });

/** Every way a reply can end early, said as the person can act on it. */
export function faultView(reason: PageAssistFaultV1): FaultView {
  if (reason.reason !== "model") return pageFault(reason.reason);
  const view = modelFault(reason.fault);
  return reason.retry_after_ms === null
    ? view
    : { ...view, wait: Math.ceil(reason.retry_after_ms / 1000) };
}

function modelFault(fault: Extract<PageAssistFaultV1, { reason: "model" }>["fault"]): FaultView {
  switch (fault) {
    case "hosted_usage":
      return {
        message: m.assist_fault_hosted_usage(),
        fix: account(m.assist_fix_buy_credits()),
        retry: "no",
      };
    case "plan_required":
      return {
        message: m.work_fault_plan_required(),
        fix: account(m.work_fault_fix_plans()),
        retry: "no",
      };
    case "free_usage_paused":
      return {
        message: m.work_fault_free_usage_paused(),
        fix: account(m.work_fault_fix_plans()),
        retry: "no",
      };
    case "billing_review":
      return {
        message: m.work_fault_billing_review(),
        fix: account(m.work_fault_fix_account()),
        retry: "no",
      };
    case "daily_limit":
      return {
        message: m.work_fault_daily_limit({ time: dailyResetTime() }),
        fix: account(m.assist_fix_buy_credits()),
        retry: "no",
      };
    case "hosted_busy":
      return { message: m.work_fault_hosted_busy(), fix: null, retry: "now" };
    case "hosted_paused":
      return {
        message: m.work_fault_hosted_paused(),
        fix: models(m.work_fault_fix_key()),
        retry: "no",
      };
    case "signed_out":
      return {
        message: m.work_fault_signed_out(),
        fix: account(m.work_fault_fix_sign_in()),
        retry: "no",
      };
    case "missing_key":
      return {
        message: m.assist_fault_missing_key(),
        fix: models(m.assist_fix_open_models()),
        retry: "no",
      };
    case "key_refused":
      return {
        message: m.work_fault_key_refused(),
        fix: models(m.assist_fix_open_models()),
        retry: "no",
      };
    case "provider_credit":
      return {
        message: m.work_fault_provider_credit(),
        fix: models(m.assist_fix_open_models()),
        retry: "no",
      };
    case "plan_limit":
      return {
        message: m.work_fault_plan_limit(),
        fix: models(m.assist_fix_open_models()),
        retry: "no",
      };
    case "rate_limited":
      return { message: m.assist_fault_rate_limited(), fix: null, retry: "wait" };
    case "provider_down":
      return { message: m.assist_fault_provider_down(), fix: null, retry: "now" };
    case "unreachable":
      return { message: m.assist_fault_unreachable(), fix: null, retry: "now" };
    case "too_large":
      return { message: m.assist_fault_too_large(), fix: null, retry: "no" };
  }
}

function pageFault(reason: Exclude<PageAssistFaultV1["reason"], "model">): FaultView {
  switch (reason) {
    case "unreadable":
      return { message: m.assist_fault_unreadable(), fix: null, retry: "now" };
    case "page_changed":
      return { message: m.assist_fault_page_changed(), fix: null, retry: "now" };
    case "no_selection":
      return { message: m.assist_fault_no_selection(), fix: null, retry: "no" };
    case "no_answer":
      return { message: m.assist_fault_no_answer(), fix: null, retry: "now" };
    case "step_failed":
      return { message: m.assist_fault_step_failed(), fix: null, retry: "now" };
    case "too_long":
      return { message: m.assist_fault_too_long(), fix: null, retry: "now" };
    case "busy":
      return { message: m.assist_fault_busy(), fix: null, retry: "no" };
    case "no_model":
      return {
        message: m.assist_refusal_no_model_help(),
        fix: models(m.assist_fix_open_models()),
        retry: "no",
      };
    case "ai_off":
    case "private":
    case "not_web":
    case "sensitive":
    case "unsupported":
      return { message: availabilityView(reason).help, fix: null, retry: "no" };
    case "loading":
      return { message: m.assist_fault_loading(), fix: null, retry: "now" };
    case "invalid":
      return { message: m.assist_fault_invalid(), fix: null, retry: "no" };
    case "unavailable":
      return { message: m.assist_unavailable(), fix: null, retry: "now" };
  }
}

export type AvailabilityView = {
  title: string;
  help: string;
  fix: FaultFix | null;
  /** The conversation still works without the page. */
  talks: boolean;
};

/** Why the page in front is not attached, and whether the conversation still goes on. */
export function availabilityView(
  availability: Exclude<PageAssistAvailabilityV1, "ready">,
): AvailabilityView {
  switch (availability) {
    case "ai_off":
      return {
        title: m.assist_refusal_off(),
        help: m.assist_refusal_off_help(),
        fix: null,
        talks: false,
      };
    case "private":
      return {
        title: m.assist_refusal_private(),
        help: m.assist_refusal_private_help(),
        fix: null,
        talks: false,
      };
    case "no_model":
      return {
        title: m.assist_refusal_no_model(),
        help: m.assist_refusal_no_model_help(),
        fix: models(m.assist_fix_open_models()),
        talks: false,
      };
    case "sensitive":
      return {
        title: m.assist_refusal_sensitive(),
        help: m.assist_refusal_sensitive_help(),
        fix: null,
        talks: true,
      };
    case "not_web":
      return {
        title: m.assist_refusal_not_page(),
        help: m.assist_refusal_not_page_help(),
        fix: null,
        talks: true,
      };
    case "unsupported":
      return {
        title: m.assist_refusal_unsupported(),
        help: m.assist_refusal_unsupported_help(),
        fix: null,
        talks: true,
      };
    case "loading":
      return {
        title: m.assist_refusal_loading(),
        help: m.assist_fault_loading(),
        fix: null,
        talks: true,
      };
    case "unavailable":
      return {
        title: m.assist_refusal_unavailable(),
        help: m.assist_refusal_unavailable_help(),
        fix: null,
        talks: true,
      };
  }
}
