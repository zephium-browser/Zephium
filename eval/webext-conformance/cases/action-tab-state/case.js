async function state(tabId) {
  return {
    title: await chrome.action.getTitle({ tabId }),
    badge: await chrome.action.getBadgeText({ tabId }),
    color: await chrome.action.getBadgeBackgroundColor({ tabId }),
    enabled: await chrome.action.isEnabled(tabId),
  };
}

export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html?action-first`);
  try {
    await h.call("set global title", () =>
      chrome.action.setTitle({ title: "Global title" }),
    );
    await h.call("set global badge", () =>
      chrome.action.setBadgeText({ text: "G" }),
    );
    await h.call("set global color", () =>
      chrome.action.setBadgeBackgroundColor({ color: "#123456" }),
    );
    await h.call("global inherited state", () => state(tab.id));
    await h.call("set tab title", () =>
      chrome.action.setTitle({ tabId: tab.id, title: "Tab title" }),
    );
    await h.call("set tab badge", () =>
      chrome.action.setBadgeText({ tabId: tab.id, text: "T" }),
    );
    await h.call("set tab color", () =>
      chrome.action.setBadgeBackgroundColor({
        tabId: tab.id,
        color: [1, 2, 3, 255],
      }),
    );
    await h.call("disable tab action", () => chrome.action.disable(tab.id));
    await h.call("tab override state", () => state(tab.id));
    const complete = h.event(
      chrome.tabs.onUpdated,
      (id, change) => id === tab.id && change.status === "complete",
    );
    await chrome.tabs.update(tab.id, {
      url: `${h.origins.b}/page.html?action-second`,
    });
    await complete;
    await h.call("state after navigation", () => state(tab.id));
    await h.call("enable tab action", () => chrome.action.enable(tab.id));
    await h.call("enabled state", () => chrome.action.isEnabled(tab.id));
  } finally {
    await chrome.tabs.remove(tab.id);
  }
}
