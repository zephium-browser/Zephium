async function navigate(h, tabId, url) {
  const complete = h.event(
    chrome.tabs.onUpdated,
    (id, change) => id === tabId && change.status === "complete",
  );
  await chrome.tabs.update(tabId, { url });
  await complete;
}

export async function run(h) {
  const tab = await h.openTab(`${h.origins.b}/page.html?active-tab-before`);
  const execute = () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => ({ url: location.href, title: document.title }),
    });
  try {
    await h.call("execute before action", execute);
    const click = h
      .event(
        chrome.storage.onChanged,
        (changes, area) => area === "local" && Boolean(changes.actionClick),
        7000,
      )
      .catch(() => null);
    const invocation = await h.call("invoke real action", () =>
      h.host("invoke-action", {
        tabId: tab.id,
        url: `${h.origins.b}/page.html?active-tab-before`,
      }),
    );
    h.assert(
      invocation && invocation.supported !== false,
      invocation?.reason ||
        "The runner did not provide a real action invocation.",
    );
    const observed = await click;
    h.record("action click observed", Boolean(observed));
    h.assert(
      observed,
      "The browser action invocation did not dispatch action.onClicked.",
    );
    await h.call("action click tab", () =>
      chrome.storage.local.get("actionClick"),
    );
    await h.call("execute after action", execute);
    await navigate(
      h,
      tab.id,
      `${h.origins.a}/page.html?active-tab-cross-origin`,
    );
    await navigate(h, tab.id, `${h.origins.b}/page.html?active-tab-revisited`);
    await h.call("execute after cross-origin navigation", execute);
  } finally {
    await chrome.tabs.remove(tab.id);
  }
}
