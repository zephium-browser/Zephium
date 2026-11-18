function view(tab) {
  const keys = [
    "id",
    "windowId",
    "index",
    "active",
    "pinned",
    "status",
    "url",
    "pendingUrl",
    "title",
  ];
  return Object.fromEntries(
    keys.filter((key) => Object.hasOwn(tab, key)).map((key) => [key, tab[key]]),
  );
}

export async function run(h) {
  const tab = await h.openTab(`${h.origins.b}/page.html?tab=metadata`);
  try {
    await h.call("get with tabs permission", async () =>
      view(await chrome.tabs.get(tab.id)),
    );
    await h.call("pin tab", async () => {
      const value = await chrome.tabs.update(tab.id, { pinned: true });
      return { tabId: value.id, pinned: value.pinned };
    });
    await h.call("query combined filters", async () =>
      (
        await chrome.tabs.query({
          url: `${h.origins.b}/*`,
          pinned: true,
          active: false,
          windowId: tab.windowId,
        })
      ).map(view),
    );
    await h.call("query unmatched URL", () =>
      chrome.tabs.query({
        url: `${h.origins.b}/never-matches-*`,
        pinned: true,
      }),
    );
    await h.call("query invalid window", () =>
      chrome.tabs.query({ windowId: -999 }),
    );
  } finally {
    await chrome.tabs.remove(tab.id);
  }
}
