export async function run(h) {
  const idle = h.event(
    chrome.runtime.onMessage,
    (message) => message?.probe === "run-at-idle",
  );
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  await idle;
  await h.call("isolated stages", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => globalThis.injectionStages,
    }),
  );
  await h.call("main stages", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      world: "MAIN",
      func: () => globalThis.injectionStages,
    }),
  );
}
