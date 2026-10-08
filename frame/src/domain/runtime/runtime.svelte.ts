import type { RuntimeStatus } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";

let state = $state.raw<RuntimeStatus>({
  restart_required: false,
  session_set_aside: false,
  user_content_degraded_scope_count: 0,
  security_advisories: [],
});

let lifecycle = 0;
let initialized = false;
let initializing: Promise<void> | null = null;
let unlisten: (() => void) | null = null;

export const status = () => state;

async function initialize(generation: number) {
  const stop = await events.runtimeStatusChanged.listen((event) => {
    if (generation === lifecycle) state = event.payload;
  });

  if (generation !== lifecycle) {
    stop();
    return;
  }

  unlisten = stop;
  initialized = true;
}

export function init(): Promise<void> {
  if (initializing !== null) return initializing;
  if (initialized) return Promise.resolve();

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
  if (!initialized && initializing === null && unlisten === null) return;

  lifecycle += 1;
  initialized = false;
  initializing = null;
  unlisten?.();
  unlisten = null;
  state = {
    restart_required: false,
    session_set_aside: false,
    user_content_degraded_scope_count: 0,
    security_advisories: [],
  };
}
