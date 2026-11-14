export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  try {
    const port = chrome.tabs.connect(tab.id, {
      name: "navigation-lifetime",
      frameId: 0,
    });
    h.record("port.ready", (await h.event(port.onMessage))[0]);
    const echo = h.event(port.onMessage);
    port.postMessage({ sequence: 1 });
    h.record("port.echo", (await echo)[0]);
    const disconnected = new Promise((resolve) =>
      port.onDisconnect.addListener((value) =>
        resolve({
          samePort: value === port,
          lastError: chrome.runtime.lastError?.message ?? null,
        }),
      ),
    );
    const complete = h.event(
      chrome.tabs.onUpdated,
      (tabId, change) => tabId === tab.id && change.status === "complete",
    );
    await chrome.tabs.update(tab.id, {
      url: `${h.origins.b}/page.html?navigated=1`,
    });
    h.record("port.navigationDisconnect", await disconnected);
    await complete;
    await h.call("port.postAfterNavigation", () =>
      port.postMessage({ sequence: 2 }),
    );
  } finally {
    await chrome.tabs.remove(tab.id);
  }
}
