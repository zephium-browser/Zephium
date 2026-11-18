export async function run(h) {
  const url = `${h.origins.a}/resource.txt?dnr-redirect`;
  await h.call("install redirect", () =>
    chrome.declarativeNetRequest.updateDynamicRules({
      addRules: [
        {
          id: 21,
          priority: 1,
          action: {
            type: "redirect",
            redirect: { extensionPath: "/redirected.txt" },
          },
          condition: {
            urlFilter: `|${url}|`,
            resourceTypes: ["xmlhttprequest"],
          },
        },
      ],
    }),
  );
  h.assert(
    (await chrome.declarativeNetRequest.getDynamicRules()).some(
      (rule) => rule.id === 21,
    ),
    "The extension redirect rule was not installed.",
  );
  await h.call("redirect fetch", async () => {
    const response = await fetch(url, { cache: "no-store" });
    return {
      status: response.status,
      url: response.url,
      redirected: response.redirected,
      text: await response.text(),
    };
  });
  await chrome.declarativeNetRequest.updateDynamicRules({
    removeRuleIds: [21],
  });
}
