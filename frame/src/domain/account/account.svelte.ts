/**
 * The optional Zephium account, one per install. Rust owns the session, the
 * vault and every outcome; this mirrors its last answer. Codes and addresses
 * pass through once and are not kept here.
 */
import {
  commands,
  type AccountAvatarCropV1,
  type AccountFaultV1,
  type AccountHistoryV1,
  type AccountPurchaseV1,
  type AccountReplyV1,
  type AccountRequestV1,
  type AccountSessionV1,
  type AccountV1,
} from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";
import { observe } from "$shared/lib/observe";

const TIMEOUT = 15_000;
/** Signing out tells the server first, and must not hang on it. */
const SIGN_OUT_TIMEOUT = 30_000;
/** Export waits on the person choosing where to save. */
const EXPORT_TIMEOUT = 10 * 60_000;
/** The picture is uploaded, converted by the service, then fetched again. */
const AVATAR_TIMEOUT = 60_000;

type AccountAction =
  | "send"
  | "verify"
  | "sign_out"
  | "sign_out_all"
  | "buy"
  | "manage"
  | "revoke"
  | "server"
  | "deletion_code"
  | "delete"
  | "name"
  | "reauth"
  | "email_code"
  | "email_change"
  | "limit"
  | "plan_preview"
  | "plan_confirm"
  | "plan_dismiss"
  | "export"
  | "avatar"
  | "avatar_remove";

type AccountFault = {
  action: AccountAction;
  fault: AccountFaultV1;
  /** With `rate_limited`, seconds until another try. */
  retryAfter: number | null;
};

let current = $state.raw<AccountV1 | null>(null);
let unreachable = $state(false);
let busy = $state<AccountAction | null>(null);
let lastFault = $state.raw<AccountFault | null>(null);
let moved = $state<string | null>(null);
// Only the settled form stage survives navigation, never the emailed code.
let deletionCodeSent = $state(false);
let users = 0;
let stop: (() => void) | null = null;
let generation = 0;
let reads = 0;

export const account = () => current;
export const unavailable = () => unreachable || (current !== null && !current.available);
export const pending = () => busy;
export const fault = () => lastFault;
export const awaitingDeletionCode = () => deletionCodeSent;

function adopt(reply: AccountReplyV1 | undefined) {
  if (reply?.account?.version !== 1) {
    unreachable = true;
    return;
  }
  unreachable = false;
  if (
    !reply.account.signed_in ||
    current?.server !== reply.account.server ||
    current?.email !== reply.account.email
  )
    deletionCodeSent = false;
  current = reply.account;
  if (current.signed_in) moved = null;
}

async function read() {
  const sequence = ++reads;
  const started = generation;
  const response = await observe(
    Promise.resolve().then(() => commands.accountGet()),
    TIMEOUT,
  );
  if (started !== generation || sequence !== reads) return;
  if (response.state !== "received") {
    if (!current) unreachable = true;
    return;
  }
  adopt(response.value);
}

/**
 * Starts mirroring; each user calls `release` when done. The listener is in
 * place before the first read, so a change in between is never lost.
 */
export async function retain() {
  users += 1;
  if (users > 1) {
    // A first read that failed is retried by the next user.
    if (!current) await read();
    return;
  }
  const started = ++generation;
  const unlisten = await events.accountChanged.listen(() => void read());
  if (started !== generation || users === 0) {
    unlisten();
    return;
  }
  stop = unlisten;
  await read();
}

export function release() {
  users = Math.max(0, users - 1);
  if (users > 0) return;
  generation += 1;
  stop?.();
  stop = null;
  busy = null;
  lastFault = null;
  moved = null;
}

async function act(
  action: AccountAction,
  request: () => Promise<AccountReplyV1>,
  timeout = TIMEOUT,
): Promise<boolean> {
  if (busy) return false;
  busy = action;
  lastFault = null;
  reads += 1;
  const started = generation;
  const response = await observe(Promise.resolve().then(request), timeout);
  if (started !== generation) return false;
  busy = null;
  if (response.state !== "received" || !response.value?.account) {
    lastFault = { action, fault: "offline", retryAfter: null };
    return false;
  }
  adopt(response.value);
  const failed = response.value.fault;
  if (failed) lastFault = { action, fault: failed, retryAfter: response.value.retry_after };
  else if (action === "deletion_code")
    deletionCodeSent = response.value.account.version === 1 && response.value.account.signed_in;
  else if (action === "delete") deletionCodeSent = false;
  return !failed;
}

export function clearFault() {
  lastFault = null;
}

export const sendCode = (email: string) => act("send", () => commands.accountSendCode(email));
export const verify = (code: string) => act("verify", () => commands.accountVerify(code));
/**
 * Asks the server now; opening Account is the person choosing to. It holds
 * no action slot, so signing in is never blocked behind it.
 */
export async function refresh() {
  const started = generation;
  const response = await observe(
    Promise.resolve().then(() => commands.accountRefresh()),
    TIMEOUT,
  );
  if (started !== generation || busy) return;
  if (response.state === "received" && response.value?.account) adopt(response.value);
}
export const buy = (purchase: AccountPurchaseV1) => act("buy", () => commands.accountBuy(purchase));
export const manage = () => act("manage", () => commands.accountManage());
/** The daily limit in units, or null to remove it. */
export const setDailyLimit = (limit: string | null) =>
  act("limit", () => commands.accountSetDailyLimit(limit));
/** Asks for a quote to move to another paid plan; nothing is charged. */
export const previewPlanChange = (plan: string, interval: string) =>
  act("plan_preview", () => commands.accountPlanPreview(plan, interval));
export const confirmPlanChange = () => act("plan_confirm", () => commands.accountPlanConfirm());
export const dismissPlanChange = () => act("plan_dismiss", () => commands.accountPlanDismiss());
export const setServer = (server: string | null) =>
  act("server", () => commands.accountSetServer(server));
export const sendDeletionCode = () => act("deletion_code", () => commands.accountDeletionCode());
export const deleteAccount = (code: string) => act("delete", () => commands.accountDelete(code));
export const signOut = () => act("sign_out", () => commands.accountSignOut(), SIGN_OUT_TIMEOUT);
export const signOutEverywhere = () =>
  act("sign_out_all", () => commands.accountSignOutEverywhere(), SIGN_OUT_TIMEOUT);
export const revoke = (id: string) => act("revoke", () => commands.accountRevokeSession(id));
export const setName = (name: string | null) => act("name", () => commands.accountSetName(name));
/** Emails a sign-in code to the account's own address, for a change that needs a fresh session. */
export const sendReauthCode = () => act("reauth", () => commands.accountReauthCode());
export const sendEmailCode = (email: string) =>
  act("email_code", () => commands.accountEmailCode(email));

/** The address the account just moved to, until signing in again. */
export const movedTo = () => moved;

/**
 * Moves the account to `email`. The server ends every session when it does,
 * so a sign-in code goes to the new address straight away.
 */
export async function changeEmail(email: string, code: string): Promise<boolean> {
  if (!(await act("email_change", () => commands.accountChangeEmail(email, code)))) return false;
  moved = email;
  await sendCode(email);
  return true;
}

/** Saves the account's export where the person chooses. */
export async function exportData(): Promise<"saved" | "cancelled" | AccountFaultV1> {
  if (busy) return "cancelled";
  busy = "export";
  lastFault = null;
  const started = generation;
  const response = await observe(
    Promise.resolve().then(() => commands.accountExport()),
    EXPORT_TIMEOUT,
  );
  if (started !== generation) return "cancelled";
  busy = null;
  if (response.state !== "received") return "offline";
  if (response.value.fault) return response.value.fault;
  return response.value.saved ? "saved" : "cancelled";
}

export async function usageHistory(cursor: string | null): Promise<AccountHistoryV1> {
  const response = await observe(
    Promise.resolve().then(() => commands.accountUsageHistory(cursor)),
    TIMEOUT,
  );
  if (response.state === "received") return response.value;
  return { requests: [], next_cursor: null, fault: "offline" };
}

export async function receipt(id: string): Promise<AccountRequestV1 | AccountFaultV1> {
  const response = await observe(
    Promise.resolve().then(() => commands.accountReceipt(id)),
    TIMEOUT,
  );
  if (response.state !== "received") return "offline";
  return response.value.request ?? response.value.fault ?? "server";
}

export type AvatarPick =
  { token: string; width: number; height: number } | "cancelled" | "unsupported" | "unavailable";

/** Opens the system's picker; the picture is staged natively for framing. */
export async function pickAvatar(): Promise<AvatarPick> {
  const reply = await observe(
    Promise.resolve().then(() => commands.accountAvatarPick()),
    10 * 60_000,
  );
  if (reply.state !== "received" || reply.value?.status !== "ok") return "unavailable";
  const pick = reply.value.data;
  return pick.kind === "picked"
    ? { token: pick.token, width: pick.width, height: pick.height }
    : pick.kind;
}

/** Uploads the framed square of a staged picture as the account's picture. */
export const saveAvatar = (token: string, crop: AccountAvatarCropV1) =>
  act("avatar", () => commands.accountAvatarSave(token, crop), AVATAR_TIMEOUT);
/** Removes the picture on every device; an upload under way is cancelled first. */
export const removeAvatar = () => act("avatar_remove", () => commands.accountAvatarRemove());

/** Puts a staged picture away, stopping its upload if one is under way. */
export async function discardAvatar(token: string) {
  await observe(
    Promise.resolve().then(() => commands.accountAvatarDiscard(token)),
    TIMEOUT,
  );
}

/** Goes back to entering an address; nothing is sent. */
export async function cancelCode() {
  lastFault = null;
  const response = await observe(
    Promise.resolve().then(() => commands.accountCancelCode()),
    TIMEOUT,
  );
  if (response.state === "received") adopt(response.value);
}

export async function sessions(): Promise<AccountSessionV1[] | AccountFaultV1> {
  const response = await observe(
    Promise.resolve().then(() => commands.accountSessions()),
    TIMEOUT,
  );
  if (response.state !== "received") return "offline";
  return response.value.fault ?? response.value.sessions;
}
