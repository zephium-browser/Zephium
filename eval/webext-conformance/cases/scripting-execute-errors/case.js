export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  await h.call("function throws", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => {
        throw new Error("injected sync failure");
      },
    }),
  );
  await h.call("function rejects", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: async () => {
        throw new Error("injected async failure");
      },
    }),
  );
  await h.call("unknown frame", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id, frameIds: [2147483647] },
      func: () => true,
    }),
  );
  await h.call("files and function conflict", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      files: ["empty.js"],
      func: () => true,
    }),
  );
  await h.call("frameIds and allFrames conflict", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id, allFrames: true, frameIds: [0] },
      func: () => true,
    }),
  );
  await h.call("missing file", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      files: ["missing.js"],
    }),
  );
  await chrome.tabs.remove(tab.id);
  await h.call("closed tab", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => true,
    }),
  );
}
