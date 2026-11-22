// This body runs only in WebKit's native background page, whose service-worker
// client can unregister its own worker. Ordinary extension pages cannot do so.
const script = new URL(expected);
const here = new URL(location.href);
const sameOrigin = (a, b) =>
  a.protocol === b.protocol && a.hostname === b.hostname && a.port === b.port;
if (!sameOrigin(script, here)) throw new Error("Worker origin mismatch");
const registration = await navigator.serviceWorker.getRegistration(expected);
if (
  !registration?.active ||
  registration.active.scriptURL !== expected ||
  !sameOrigin(script, new URL(registration.scope))
) {
  throw new Error("Worker registration mismatch");
}
const scope = registration.scope;
if (!(await registration.unregister()))
  throw new Error("Worker unregister failed");
const replacement = await navigator.serviceWorker.register(expected, {
  scope,
  type: modules ? "module" : "classic",
});
const worker =
  replacement.installing ?? replacement.waiting ?? replacement.active;
if (!worker) throw new Error("Worker replacement missing");
await new Promise((resolve, reject) => {
  const changed = () => {
    if (worker.state === "activated" || worker.state === "redundant") {
      worker.removeEventListener("statechange", changed);
      if (worker.state === "activated") resolve();
      else reject(new Error("Worker replacement failed"));
    }
  };
  worker.addEventListener("statechange", changed);
  changed();
});
return true;
