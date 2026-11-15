export async function run(h) {
  const url = `${h.origins.a}/page.html?dnr-upgrade`;
  await h.call("install upgrade", () =>
    chrome.declarativeNetRequest.updateDynamicRules({
      addRules: [
        {
          id: 31,
          priority: 1,
          action: { type: "upgradeScheme" },
          condition: { urlFilter: `|${url}|`, resourceTypes: ["main_frame"] },
        },
      ],
    }),
  );
  h.assert(
    (await chrome.declarativeNetRequest.getDynamicRules()).some(
      (rule) => rule.id === 31,
    ),
    "The scheme upgrade rule was not installed.",
  );
  const tab = await h.openTab("about:blank");
  try {
    const failed = h.event(
      chrome.webNavigation.onErrorOccurred,
      (details) => details.tabId === tab.id && details.frameId === 0,
    );
    await h.call("navigate to HTTP fixture", async () => {
      const value = await chrome.tabs.update(tab.id, { url });
      return { tabId: value.id };
    });
    const [details] = await failed;
    h.record("navigation failure on HTTP-only fixture", {
      tabId: details.tabId,
      frameId: details.frameId,
      url: details.url,
      error: details.error,
      upgraded: details.url.startsWith("https:"),
    });
  } finally {
    await chrome.tabs.remove(tab.id);
    await chrome.declarativeNetRequest.updateDynamicRules({
      removeRuleIds: [31],
    });
  }
}
