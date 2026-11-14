export async function run(h) {
  const facts = await h.call("service worker facts", () =>
    chrome.runtime.sendMessage({ probe: "worker-facts" }),
  );
  h.assert(
    facts?.isServiceWorker,
    "The classic service worker did not return its environment.",
  );
}
