export async function run(h) {
  await h.callback("platform.callback", (done) =>
    chrome.runtime.getPlatformInfo(done),
  );
  await h.call("platform.promise", () => chrome.runtime.getPlatformInfo());
  await h.callback("noReceiver.callback", (done) =>
    chrome.runtime.sendMessage({ probe: "no-receiver" }, done),
  );
  h.record("lastError.afterCallback", {
    present: chrome.runtime.lastError !== undefined,
  });
  await h.call("noReceiver.promise", () =>
    chrome.runtime.sendMessage({ probe: "no-receiver" }),
  );
  h.record("lastError.afterPromise", {
    present: chrome.runtime.lastError !== undefined,
  });
}
