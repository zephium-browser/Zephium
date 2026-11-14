export async function run(h) {
  const port = chrome.runtime.connect({ name: "background-lifetime" });
  h.record("port.ready", (await h.event(port.onMessage))[0]);
  const disconnected = new Promise((resolve) =>
    port.onDisconnect.addListener((value) =>
      resolve({
        samePort: value === port,
        lastError: chrome.runtime.lastError?.message ?? null,
      }),
    ),
  );
  const stopped = await h.host("stopBackground", {});
  h.assert(stopped.stopped === true, "The runner stopped the service worker");
  h.record("background.stop", stopped);
  h.record("port.backgroundDisconnect", await disconnected);
  await h.call("port.postAfterBackgroundStop", () =>
    port.postMessage({ sequence: 1 }),
  );
}
