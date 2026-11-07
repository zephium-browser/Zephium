import type {
  ExtensionActionFailedView,
  ExtensionActionShortcutView,
  ExtensionActionFailure,
  ExtensionActionView,
  ExtensionActionsView,
} from "$shared/ipc/bindings";
import { SvelteSet } from "svelte/reactivity";
import { commands } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import {
  ExtensionActionShortcutProjectionModel,
  ExtensionProjectionModel,
  failureForContext,
} from "./action-model";
export { chromeStoreListingId, isChromeStoreListing } from "./store-listing";

const NOTICE_LIFETIME_MS = 5_000;
const ACTION_SHORTCUT_LIFETIME_MS = 1_000;

const model = new ExtensionProjectionModel();
const shortcutModel = new ExtensionActionShortcutProjectionModel();
let state = $state.raw<ExtensionActionsView>(model.actions);
let shortcutState = $state.raw<ExtensionActionShortcutView | null>(shortcutModel.view);
type VisibleFailure = Omit<ExtensionActionFailedView, "projection_revision"> & {
  projectionRevision?: string;
};
let failure = $state.raw<VisibleFailure | null>(null);
const invoking = new SvelteSet<string>();

let lifecycle = 0;
let initialized = false;
let initializing: Promise<void> | null = null;
let unlisten: (() => void) | null = null;
let noticeTimer: ReturnType<typeof setTimeout> | null = null;
let shortcutTimer: ReturnType<typeof setTimeout> | null = null;

type Unlisten = () => void;

export const failureReason = (profileId: string | null, tabId: string | null) =>
  failureForContext(failure, profileId, tabId);
export const isInvoking = (installId: string) => invoking.has(installId);
export const actionShortcut = (profileId: string | null, tabId: string | null) =>
  shortcutState !== null && shortcutState.profile_id === profileId && shortcutState.tab_id === tabId
    ? shortcutState
    : null;
export const consumeActionShortcut = (revision: string) => {
  const consumed = shortcutModel.consume(revision);
  if (consumed) {
    if (shortcutTimer !== null) clearTimeout(shortcutTimer);
    shortcutTimer = null;
    shortcutState = shortcutModel.view;
  }
  return consumed;
};

export function activeActions(profileId: string | null, tabId: string | null) {
  return profileId !== null &&
    tabId !== null &&
    state.profile_id === profileId &&
    state.tab_id === tabId
    ? state.actions
    : [];
}

function clearVisibleFailure() {
  if (noticeTimer !== null) clearTimeout(noticeTimer);
  noticeTimer = null;
  failure = null;
}

function publishFailure(
  reason: ExtensionActionFailure,
  profileId: string,
  tabId: string,
  revision?: string,
) {
  failure = { reason, profile_id: profileId, tab_id: tabId, projectionRevision: revision };
  if (noticeTimer !== null) clearTimeout(noticeTimer);
  const generation = lifecycle;
  noticeTimer = setTimeout(() => {
    noticeTimer = null;
    if (generation !== lifecycle) return;
    if (revision !== undefined) model.clearFailure(revision);
    failure = null;
  }, NOTICE_LIFETIME_MS);
}

async function resolveListeners(promises: readonly Promise<Unlisten>[]): Promise<Unlisten[]> {
  const results = await Promise.allSettled(promises);
  const listeners: Unlisten[] = [];
  let failed = false;
  let failureReason: unknown;
  for (const result of results) {
    if (result.status === "fulfilled") listeners.push(result.value);
    else if (!failed) {
      failed = true;
      failureReason = result.reason;
    }
  }
  if (failed) {
    for (const stop of listeners) stop();
    throw failureReason;
  }
  return listeners;
}

async function initialize(generation: number) {
  const listeners = await resolveListeners([
    events.extensionActionsChanged.listen((event) => {
      if (generation !== lifecycle || !model.applyActions(event.payload)) return;
      state = model.actions;
      shortcutModel.observeContextRevision(model.revision);
      shortcutState = shortcutModel.view;
      if (shortcutState === null && shortcutTimer !== null) {
        clearTimeout(shortcutTimer);
        shortcutTimer = null;
      }
      clearVisibleFailure();
    }),
    events.extensionActionFailed.listen((event) => {
      if (generation !== lifecycle || !model.applyFailure(event.payload)) return;
      shortcutModel.observeContextRevision(model.revision);
      shortcutState = shortcutModel.view;
      if (shortcutState === null && shortcutTimer !== null) {
        clearTimeout(shortcutTimer);
        shortcutTimer = null;
      }
      publishFailure(
        event.payload.reason,
        event.payload.profile_id,
        event.payload.tab_id,
        event.payload.projection_revision,
      );
    }),
    events.extensionActionShortcut.listen((event) => {
      if (generation !== lifecycle || !shortcutModel.apply(event.payload, model.revision)) return;
      shortcutState = shortcutModel.view;
      if (shortcutTimer !== null) clearTimeout(shortcutTimer);
      const revision = event.payload.projection_revision;
      shortcutTimer = setTimeout(() => {
        shortcutTimer = null;
        if (generation !== lifecycle || !shortcutModel.consume(revision)) return;
        shortcutState = shortcutModel.view;
      }, ACTION_SHORTCUT_LIFETIME_MS);
    }),
  ]);
  if (generation !== lifecycle) {
    for (const stop of listeners) stop();
    return;
  }
  unlisten = () => {
    for (const stop of listeners) stop();
  };
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
  if (noticeTimer !== null) clearTimeout(noticeTimer);
  noticeTimer = null;
  if (shortcutTimer !== null) clearTimeout(shortcutTimer);
  shortcutTimer = null;
  invoking.clear();
  shortcutModel.clear();
  shortcutState = shortcutModel.view;
  failure = null;
}

/** The button's context menu: the extension's items for its action. */
export function menu(profileId: string, action: ExtensionActionView, anchor: DOMRect): void {
  void commands
    .extensionActionMenu(
      profileId,
      action.runtime.install_id,
      action.runtime.generation,
      action.revision,
      anchor.left,
      anchor.top,
      anchor.width,
      anchor.height,
    )
    .catch(() => {});
}

export async function invoke(
  profileId: string,
  tabId: string,
  action: ExtensionActionView,
  anchor: DOMRect,
): Promise<void> {
  const key = action.runtime.install_id;
  if (!action.enabled || invoking.has(key)) return;
  invoking.add(key);
  try {
    const admission = await commands.extensionActionInvoke(
      profileId,
      action.runtime.install_id,
      action.runtime.generation,
      action.revision,
      anchor.left,
      anchor.top,
      anchor.width,
      anchor.height,
    );
    if (!admission.accepted) publishFailure("invalid_request", profileId, tabId);
  } catch {
    publishFailure("native_admission_failed", profileId, tabId);
  } finally {
    invoking.delete(key);
  }
}
