import type {
  OperationDisposition,
  PagePermissionPromptDecisionInput,
  PagePermissionPromptEntryView,
  PagePermissionPromptView,
} from "$shared/ipc/bindings";
import * as m from "$shared/i18n/messages";
import { commands } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { operations } from "$domain/operations";
import {
  PagePermissionPromptProjectionModel,
  initialPagePermissionPrompt,
} from "./page-permissions-model";

const IPC_TIMEOUT_MS = 5_000;
const SETTLEMENT_TIMEOUT_MS = 30_000;

const model = new PagePermissionPromptProjectionModel();
let state = $state.raw<PagePermissionPromptView>(model.view);
let responding = $state(false);
let notice = $state<string | null>(null);
let lifecycle = 0;
let initialized = false;
let initializing: Promise<void> | null = null;
let unlisten: (() => void) | null = null;

export const prompt = () => state.prompt;
export const busy = () => responding || state.prompt?.processing === true;
export const failure = () => notice;

function samePrompt(
  left: PagePermissionPromptEntryView | null,
  right: PagePermissionPromptEntryView,
): boolean {
  return (
    left?.profile_id === right.profile_id &&
    left.item_id === right.item_id &&
    left.request_id === right.request_id
  );
}

async function boundedIpc<T>(request: Promise<T>, milliseconds: number): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    return await Promise.race([
      request,
      new Promise<T>((_resolve, reject) => {
        timer = setTimeout(() => reject(new Error("bounded privileged IPC timeout")), milliseconds);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

function dispositionMessage(disposition: OperationDisposition): string | null {
  if (disposition.outcome === "applied" || disposition.outcome === "no_op") return null;
  switch (disposition.reason) {
    case "invalid_scope":
      return m.page_permission_unavailable();
    case "store_conflict":
      return m.page_permission_conflict();
    case "store_outcome_unknown":
    case "store_reconciliation_failed":
      return m.page_permission_unverified_store();
    case "store_admission_rejected":
      return m.page_permission_store_unavailable();
    default:
      return m.page_permission_apply_failed();
  }
}

async function initialize(generation: number) {
  const stop = await events.pagePermissionPromptChanged.listen((event) => {
    if (generation !== lifecycle || !model.apply(event.payload)) return;
    state = model.view;
    responding = false;
    notice = null;
  });
  if (generation !== lifecycle) {
    stop();
    return;
  }
  unlisten = stop;
  initialized = true;
}

export function init(): Promise<void> {
  if (initialized) return Promise.resolve();
  if (initializing !== null) return initializing;
  const generation = ++lifecycle;
  const task = initialize(generation);
  initializing = task;
  void task.then(
    () => {
      if (initializing === task) initializing = null;
    },
    () => {
      if (initializing === task) initializing = null;
    },
  );
  return task;
}

export function dispose() {
  lifecycle += 1;
  initialized = false;
  initializing = null;
  unlisten?.();
  unlisten = null;
  state = initialPagePermissionPrompt();
  responding = false;
  notice = null;
}

async function settle(
  retained: PagePermissionPromptEntryView,
  decision: PagePermissionPromptDecisionInput,
): Promise<void> {
  if (!samePrompt(state.prompt, retained) || busy()) return;
  responding = true;
  notice = null;
  try {
    try {
      await operations.init();
    } catch {
      // The operation ledger remains the bounded reconciliation path if its
      // listener races this foreground gesture.
    }
    const admission = await boundedIpc(
      commands.pagePermissionRespond(
        retained.profile_id,
        retained.item_id,
        retained.request_id,
        decision,
      ),
      IPC_TIMEOUT_MS,
    );
    if (!admission.accepted || admission.operation_id === null) {
      notice = m.page_permission_unavailable();
      responding = false;
      return;
    }
    const resolution = await operations.waitForDisposition(
      admission.operation_id,
      SETTLEMENT_TIMEOUT_MS,
    );
    if (!samePrompt(state.prompt, retained)) return;
    if (resolution.state === "processed") {
      notice = dispositionMessage(resolution.disposition);
      if (notice !== null) responding = false;
    } else if (resolution.state === "pending") {
      notice = m.page_permission_still_applying();
    } else {
      notice = m.page_permission_unverified();
      responding = false;
    }
  } catch {
    if (samePrompt(state.prompt, retained)) {
      notice = m.page_permission_service_unavailable();
      responding = false;
    }
  }
}

let sidebarHosts = 0;
let reportedHost: boolean | null = null;

function reportHost() {
  // One report per task, so a remount does not flap the layout.
  queueMicrotask(() => {
    const next = sidebarHosts > 0;
    if (next === reportedHost) return;
    reportedHost = next;
    void commands.pagePermissionCardHost(next).catch(() => {
      reportedHost = null;
    });
  });
}

/**
 * Marks the expanded sidebar's address field as on screen, where a prompt
 * shows as a card with the page still visible. Returns the release.
 */
export function hostInSidebar(): () => void {
  sidebarHosts += 1;
  reportHost();
  let released = false;
  return () => {
    if (released) return;
    released = true;
    sidebarHosts -= 1;
    reportHost();
  };
}

export function respond(
  retained: PagePermissionPromptEntryView,
  decision: PagePermissionPromptDecisionInput,
): void {
  void settle(retained, decision);
}
