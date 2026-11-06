// Added after the bench's own preload in the hooked launches of
// `--youtube-hooks` only. Puts pass-through hooks on Array.prototype.push and
// Map.prototype.has, the two a page-wide hold signal would need, counts their
// calls, and records where YouTube's player makes the two calls the quick
// fixes' script waits for: a 104- or 105-byte Uint8Array pushed onto an array,
// and a lookup of "onSnackbarMessage". Every call goes on unchanged.
if (/(^|\.)youtube\.com$/.test(location.hostname)) {
  const nativePush = Array.prototype.push;
  const nativeHas = Map.prototype.has;
  const record = { pushes: 0, lookups: 0, bytes: 0, snackbar: 0, signals: [] };
  Object.defineProperty(window, '__benchHooks', { value: record });
  const site = () => String(new Error().stack ?? '').split('\n').slice(2, 8)
    .map((line) => line.replace(/https:\/\/www\.youtube\.com/g, '').slice(0, 160));
  const note = (kind, extra) => {
    if (record.signals.length < 40) {
      Reflect.apply(nativePush, record.signals, [{ kind, atMs: Math.round(performance.now()), site: site(), ...extra }]);
    }
  };
  const isHold = (value) => value instanceof Uint8Array && (value.length === 104 || value.length === 105)
    && value.byteLength === value.buffer.byteLength;
  const push = new Proxy(nativePush, {
    apply(target, self, args) {
      record.pushes += 1;
      if (isHold(args[0])) {
        record.bytes += 1;
        const head = Array.from(args[0].subarray(0, 12), (byte) => byte.toString(16).padStart(2, '0')).join('');
        note('bytes', { length: args[0].length, head });
      }
      return Reflect.apply(target, self, args);
    },
  });
  const has = new Proxy(nativeHas, {
    apply(target, self, args) {
      record.lookups += 1;
      if (args[0] === 'onSnackbarMessage') {
        record.snackbar += 1;
        note('snackbar');
      }
      return Reflect.apply(target, self, args);
    },
  });
  Array.prototype.push = push;
  Map.prototype.has = has;
  // Per-call cost of each hook over the native call, in microseconds, timed
  // in the page after the run.
  Object.defineProperty(record, 'calibrate', {
    value: () => {
      const calls = 200000;
      const time = (run) => {
        const start = performance.now();
        run();
        return performance.now() - start;
      };
      const map = new Map([['a', 1]]);
      const per = (hooked, native) => Math.round(((hooked - native) * 1000 * 1000) / calls) / 1000;
      const nativePushMs = time(() => { const list = []; for (let i = 0; i < calls; i += 1) Reflect.apply(nativePush, list, [i]); });
      const hookedPushMs = time(() => { const list = []; for (let i = 0; i < calls; i += 1) list.push(i); });
      const nativeHasMs = time(() => { for (let i = 0; i < calls; i += 1) Reflect.apply(nativeHas, map, ['a']); });
      const hookedHasMs = time(() => { for (let i = 0; i < calls; i += 1) map.has('a'); });
      return {
        calls,
        pushUs: per(hookedPushMs, nativePushMs),
        hasUs: per(hookedHasMs, nativeHasMs),
        nativePushMs: Math.round(nativePushMs * 10) / 10,
        hookedPushMs: Math.round(hookedPushMs * 10) / 10,
        nativeHasMs: Math.round(nativeHasMs * 10) / 10,
        hookedHasMs: Math.round(hookedHasMs * 10) / 10,
      };
    },
  });
}
