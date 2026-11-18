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
  const permitted = await h.openTab(`${h.origins.a}/page.html?tab=permitted`);
  const denied = await h.openTab(`${h.origins.b}/page.html?tab=denied`);
  try {
    await h.call("get permitted", async () =>
      view(await chrome.tabs.get(permitted.id)),
    );
    await h.call("get denied", async () =>
      view(await chrome.tabs.get(denied.id)),
    );
    await h.call("query permitted URL", async () =>
      (await chrome.tabs.query({ url: `${h.origins.a}/*` }))
        .filter((tab) => tab.id === permitted.id)
        .map(view),
    );
    await h.call("query denied URL", async () =>
      (await chrome.tabs.query({ url: `${h.origins.b}/*` }))
        .filter((tab) => tab.id === denied.id)
        .map(view),
    );
    await h.call("query pinned false", async () =>
      (await chrome.tabs.query({ pinned: false }))
        .filter((tab) => tab.id === permitted.id || tab.id === denied.id)
        .map(view),
    );
  } finally {
    await chrome.tabs.remove([permitted.id, denied.id]);
  }
}
