export async function run(h) {
  await h.call("clear handler cache", () => chrome.webRequest.handlerBehaviorChanged());
  await h.call("coalesced calls", () => Promise.all(Array.from({length:4}, () => chrome.webRequest.handlerBehaviorChanged())));
  await h.call("invalid arguments", () => chrome.webRequest.handlerBehaviorChanged(42));
}
