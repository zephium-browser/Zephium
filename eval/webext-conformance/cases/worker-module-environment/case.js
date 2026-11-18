export async function run(h) {
  const facts = await h.call("service worker facts", () =>
    chrome.runtime.sendMessage({ probe: "worker-facts" }),
  );
  h.assert(
    facts?.isServiceWorker,
    "The module service worker did not return its environment.",
  );
}
