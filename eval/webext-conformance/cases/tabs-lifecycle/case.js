export async function run(h) {
  const events = [];
  let target;
  const created = (tab) => {
    if (
      tab.pendingUrl?.includes("lifecycle-first") ||
      tab.url?.includes("lifecycle-first")
    )
      events.push({ event: "created", tabId: tab.id });
  };
  const updated = (tabId, change, tab) => {
    if (
      (tabId === target || tab.url?.includes("lifecycle-first")) &&
      change.status
    )
      events.push({ event: "updated", tabId, status: change.status });
  };
  const removed = (tabId, info) => {
    if (tabId === target) events.push({ event: "removed", tabId, info });
  };
  chrome.tabs.onCreated.addListener(created);
  chrome.tabs.onUpdated.addListener(updated);
  chrome.tabs.onRemoved.addListener(removed);
  try {
    const initialComplete = h.event(
      chrome.tabs.onUpdated,
      (_id, change, tab) =>
        change.status === "complete" && tab.url?.includes("lifecycle-first"),
    );
    const tab = await h.call("create", async () => {
      const value = await chrome.tabs.create({
        url: `${h.origins.a}/page.html?lifecycle-first`,
        active: false,
      });
      return { tabId: value.id, active: value.active };
    });
    target = tab.tabId;
    await initialComplete;
    const nextComplete = h.event(
      chrome.tabs.onUpdated,
      (id, change) => id === target && change.status === "complete",
    );
    await h.call("update", async () => {
      const value = await chrome.tabs.update(target, {
        url: `${h.origins.a}/page.html?lifecycle-second`,
      });
      return { tabId: value.id };
    });
    await nextComplete;
    const removal = h.event(chrome.tabs.onRemoved, (id) => id === target);
    await h.call("remove", () => chrome.tabs.remove(target));
    await removal;
    h.record("event order", events);
    await h.call("get removed tab", () => chrome.tabs.get(target));
  } finally {
    chrome.tabs.onCreated.removeListener(created);
    chrome.tabs.onUpdated.removeListener(updated);
    chrome.tabs.onRemoved.removeListener(removed);
  }
}
