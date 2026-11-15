// This bootstrap runs only in an extension's isolated content-script world.
(() => {
  if ('requestIdleCallback' in globalThis || 'cancelIdleCallback' in globalThis) return;
  const now = performance.now.bind(performance);
  const schedule = setTimeout.bind(globalThis);
  const cancel = clearTimeout.bind(globalThis);
  const pending = new Map();
  let next = 0;
  globalThis.requestIdleCallback = function requestIdleCallback(callback, options = {}) {
    if (typeof callback !== 'function') throw new TypeError('Callback must be a function');
    const value = options?.timeout;
    const timeout = value === undefined ? Infinity : Number(value) >>> 0;
    const expires = now() + timeout;
    const handle = ++next;
    // A timer yields to the event loop; it cannot predict the renderer's idle periods.
    const timer = schedule(() => {
      pending.delete(handle);
      const start = now();
      const didTimeout = start >= expires;
      const deadline = Object.create(Object.prototype, {
        didTimeout: { enumerable: true, get: () => didTimeout },
        timeRemaining: { enumerable: true, value: () => didTimeout ? 0 : Math.max(0, 50 - (now() - start)) },
      });
      callback(deadline);
    }, Math.min(1, timeout));
    pending.set(handle, timer);
    return handle;
  };
  globalThis.cancelIdleCallback = function cancelIdleCallback(handle) {
    const key = Number(handle);
    const timer = pending.get(key);
    if (timer !== undefined) {
      pending.delete(key);
      cancel(timer);
    }
  };
})();
