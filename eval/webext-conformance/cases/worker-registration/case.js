export async function run(h) {
  await h.sleep(100);
  await h.call("registration", () => chrome.runtime.sendMessage("registration"));
  await h.call("update", () => chrome.runtime.sendMessage("update"));
  h.record("worker console", await h.host("consoleMessages", {}));
}
