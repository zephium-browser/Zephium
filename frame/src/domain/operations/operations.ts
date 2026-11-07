import { observe, pause } from "$shared/lib/observe";
import type { OperationDisposition } from "$shared/ipc/bindings";
import { commands } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";

// Rust retains every accepted operation in its bounded process-local ledger
// until this privileged WebView acknowledges the actor's disposition. Keep a
// smaller recent-result window here for UI consumers, but acknowledgement is
// independent of that presentation cache and is retried after transient IPC
// failures.
const MAX_RECENT_RESULTS = 256;
const RECONCILE_ATTEMPTS = 20;
const RECONCILE_RETRY_MS = 250;
const ACK_RETRY_MS = 1_000;
const IPC_ATTEMPT_TIMEOUT_MS = 2_000;
const MAX_ACKNOWLEDGEMENTS_IN_FLIGHT = 16;
const SETTLEMENT_POLL_MS = 1_000;
const DEFAULT_SETTLEMENT_TIMEOUT_MS = 2 * 60 * 1_000;
const OPERATION_ID = /^[0-9a-f]{16}$/;

const recent = new Map<string, OperationDisposition>();
const pendingAcks = new Set<string>();
const acknowledgementsInFlight = new Set<string>();
const settlementSignals = new Map<string, Set<() => void>>();

let lifecycle = 0;
let observationLifetime = new AbortController();
let initialized = false;
let initializing: Promise<void> | null = null;
let unlisten: (() => void) | null = null;
let ackTimer: ReturnType<typeof setTimeout> | null = null;

async function boundedIpc<T>(request: Promise<T>, milliseconds: number): Promise<T> {
  const result = await observe(request, milliseconds, observationLifetime.signal);
  if (result.state !== "received") throw new Error("native observation unavailable");
  return result.value;
}

function remember(disposition: OperationDisposition) {
  if (!recent.has(disposition.operation_id)) {
    recent.set(disposition.operation_id, disposition);
    if (recent.size > MAX_RECENT_RESULTS) {
      const oldest = recent.keys().next().value;
      if (oldest !== undefined) recent.delete(oldest);
    }
  }
  for (const signal of settlementSignals.get(disposition.operation_id) ?? []) signal();
  pendingAcks.add(disposition.operation_id);
  scheduleAcknowledgements(0);
}

function scheduleAcknowledgements(after: number) {
  if (ackTimer !== null || pendingAcks.size === 0) return;
  ackTimer = setTimeout(() => {
    ackTimer = null;
    void flushAcknowledgements();
  }, after);
}

function flushAcknowledgements() {
  const generation = lifecycle;
  const available =
    MAX_ACKNOWLEDGEMENTS_IN_FLIGHT -
    Math.min(MAX_ACKNOWLEDGEMENTS_IN_FLIGHT, acknowledgementsInFlight.size);
  const batch = [...pendingAcks]
    .filter((operationId) => !acknowledgementsInFlight.has(operationId))
    .slice(0, available);
  for (const operationId of batch) {
    acknowledgementsInFlight.add(operationId);
    void boundedIpc(commands.operationAcknowledge(operationId), IPC_ATTEMPT_TIMEOUT_MS)
      .then(() => {
        // A successful false means another delivery path already acknowledged
        // the result (or the process-local ledger was replaced). Either way,
        // there is no server entry left for this client to retain.
        pendingAcks.delete(operationId);
      })
      .catch(() => {
        // Keep the id and retry. Never turn a temporary IPC failure into an
        // ever-growing native ledger or a duplicate user-visible disposition.
      })
      .finally(() => {
        acknowledgementsInFlight.delete(operationId);
        if (generation === lifecycle) scheduleAcknowledgements(ACK_RETRY_MS);
      });
  }
}

async function reconcile(generation: number) {
  for (let attempt = 0; attempt < RECONCILE_ATTEMPTS && generation === lifecycle; attempt++) {
    try {
      const dispositions = await boundedIpc(commands.operationsReconcile(), IPC_ATTEMPT_TIMEOUT_MS);
      if (generation !== lifecycle) return;
      for (const disposition of dispositions) remember(disposition);
      return;
    } catch {
      if (generation !== lifecycle) return;
      await pause(RECONCILE_RETRY_MS, observationLifetime.signal);
    }
  }
}

async function initialize(generation: number) {
  const stop = await events.operationProcessed.listen((event) => {
    if (generation === lifecycle) remember(event.payload);
  });
  if (generation !== lifecycle) {
    stop();
    return;
  }
  unlisten = stop;
  initialized = true;
  void reconcile(generation);
  scheduleAcknowledgements(0);
}

export function init(): Promise<void> {
  if (initialized) return Promise.resolve();
  if (initializing !== null) return initializing;
  const generation = ++lifecycle;
  if (observationLifetime.signal.aborted) observationLifetime = new AbortController();
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
  observationLifetime.abort();
  initialized = false;
  initializing = null;
  unlisten?.();
  unlisten = null;
  if (ackTimer !== null) clearTimeout(ackTimer);
  ackTimer = null;
  for (const signals of settlementSignals.values()) {
    for (const signal of signals) signal();
  }
  settlementSignals.clear();
}

export const result = (operationId: string) => recent.get(operationId);

/** Observes a terminal ledger result without starting another poll or owning a surface. */
export function onDisposition(
  operationId: string,
  receive: (disposition: OperationDisposition) => void,
): () => void {
  const signal = () => {
    const disposition = recent.get(operationId);
    if (disposition) {
      try {
        receive(disposition);
      } catch {
        // A failed observer must not interrupt native ledger acknowledgement.
      }
    }
  };
  const signals = settlementSignals.get(operationId) ?? new Set<() => void>();
  signals.add(signal);
  settlementSignals.set(operationId, signals);
  signal();
  return () => {
    signals.delete(signal);
    if (signals.size === 0 && settlementSignals.get(operationId) === signals)
      settlementSignals.delete(operationId);
  };
}

export type OperationResolution =
  | { state: "processed"; disposition: OperationDisposition }
  | { state: "pending"; operation_id: string }
  | { state: "unknown"; operation_id: string };

function waitForSettlementSignal(
  operationId: string,
  milliseconds: number,
  signal?: AbortSignal,
): Promise<void> {
  return new Promise((resolve) => {
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal?.removeEventListener("abort", finish);
      const signals = settlementSignals.get(operationId);
      signals?.delete(finish);
      if (signals?.size === 0) settlementSignals.delete(operationId);
      resolve();
    };
    const timer = setTimeout(finish, milliseconds);
    const signals = settlementSignals.get(operationId) ?? new Set<() => void>();
    signals.add(finish);
    settlementSignals.set(operationId, signals);

    // Close the event-before-subscription window without another native call.
    signal?.addEventListener("abort", finish, { once: true });
    if (signal?.aborted || recent.has(operationId)) finish();
  });
}

/**
 * Waits for the actor's exact disposition while events are healthy and uses
 * the bounded desktop ledger as reconciliation fallback. A timeout reports
 * `pending`; it never turns absent evidence into success or failure.
 */
export async function waitForDisposition(
  operationId: string,
  timeoutMs = DEFAULT_SETTLEMENT_TIMEOUT_MS,
  signal?: AbortSignal,
): Promise<OperationResolution> {
  const owner = observationLifetime.signal;
  const local = new AbortController();
  const abort = () => local.abort();
  owner.addEventListener("abort", abort, { once: true });
  signal?.addEventListener("abort", abort, { once: true });
  if (owner.aborted || signal?.aborted) abort();
  try {
    return await observeDisposition(operationId, timeoutMs, local.signal);
  } finally {
    owner.removeEventListener("abort", abort);
    signal?.removeEventListener("abort", abort);
  }
}

async function observeDisposition(
  operationId: string,
  timeoutMs = DEFAULT_SETTLEMENT_TIMEOUT_MS,
  signal?: AbortSignal,
): Promise<OperationResolution> {
  if (!OPERATION_ID.test(operationId)) return { state: "unknown", operation_id: operationId };

  const cached = recent.get(operationId);
  if (cached !== undefined) return { state: "processed", disposition: cached };

  const generation = lifecycle;
  const boundedTimeout = Number.isFinite(timeoutMs)
    ? Math.max(0, Math.min(timeoutMs, DEFAULT_SETTLEMENT_TIMEOUT_MS))
    : 0;
  const deadline = Date.now() + boundedTimeout;

  while (generation === lifecycle && !signal?.aborted) {
    const remembered = recent.get(operationId);
    if (remembered !== undefined) return { state: "processed", disposition: remembered };

    try {
      const remainingBeforeQuery = deadline - Date.now();
      if (remainingBeforeQuery <= 0) break;
      const observed = await observe(
        commands.operationStatus(operationId),
        Math.min(remainingBeforeQuery, IPC_ATTEMPT_TIMEOUT_MS),
        signal,
      );
      if (generation !== lifecycle || signal?.aborted) break;
      if (observed.state !== "received") throw new Error("ledger observation unavailable");
      const status = observed.value;
      const afterQuery = recent.get(operationId);
      if (afterQuery !== undefined) return { state: "processed", disposition: afterQuery };
      if (status.state === "processed" && status.disposition.operation_id === operationId) {
        remember(status.disposition);
        return { state: "processed", disposition: status.disposition };
      }
      if (status.state === "unknown") return { state: "unknown", operation_id: operationId };
    } catch {
      // The scoped event may still deliver the disposition. Retry the ledger
      // query at a low fixed rate until the same bounded deadline.
    }

    if (generation !== lifecycle || signal?.aborted) break;
    const remaining = deadline - Date.now();
    if (remaining <= 0) break;
    await waitForSettlementSignal(operationId, Math.min(remaining, SETTLEMENT_POLL_MS), signal);
  }

  const final = recent.get(operationId);
  return final === undefined
    ? { state: "pending", operation_id: operationId }
    : { state: "processed", disposition: final };
}
