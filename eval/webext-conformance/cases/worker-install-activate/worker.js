const lifecycle = [];
self.addEventListener("install", (event) => {
  lifecycle.push("install");
  event.waitUntil(
    Promise.resolve().then(() => lifecycle.push("install settled")),
  );
});
self.addEventListener("activate", (event) => {
  lifecycle.push("activate");
  event.waitUntil(
    Promise.resolve().then(() => lifecycle.push("activate settled")),
  );
});
chrome.runtime.onInstalled.addListener((details) =>
  lifecycle.push(`runtime.onInstalled:${details.reason}`),
);
chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message.probe === "worker-lifecycle")
    sendResponse({
      events: [...lifecycle],
      state: self.registration.active?.state,
    });
});
