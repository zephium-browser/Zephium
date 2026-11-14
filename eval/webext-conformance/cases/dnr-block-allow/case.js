export async function run(h) {
  const url = `${h.origins.a}/resource.txt?dnr-priority`;
  const condition = {
    urlFilter: `|${url}|`,
    resourceTypes: ["xmlhttprequest"],
  };
  const fetchResource = async () => {
    const response = await fetch(url, { cache: "no-store" });
    return { status: response.status, text: await response.text() };
  };
  await h.call("unblocked control", fetchResource);
  await h.call("install block", () =>
    chrome.declarativeNetRequest.updateDynamicRules({
      addRules: [{ id: 11, priority: 1, action: { type: "block" }, condition }],
    }),
  );
  h.assert(
    (await chrome.declarativeNetRequest.getDynamicRules()).some(
      (rule) => rule.id === 11,
    ),
    "The blocking rule was not installed.",
  );
  await h.call("blocked fetch", fetchResource);
  await h.call("install higher priority allow", () =>
    chrome.declarativeNetRequest.updateDynamicRules({
      addRules: [{ id: 12, priority: 2, action: { type: "allow" }, condition }],
    }),
  );
  h.assert(
    (await chrome.declarativeNetRequest.getDynamicRules()).some(
      (rule) => rule.id === 12,
    ),
    "The higher priority allow rule was not installed.",
  );
  await h.call("allowed fetch", fetchResource);
  await h.call("remove allow", () =>
    chrome.declarativeNetRequest.updateDynamicRules({ removeRuleIds: [12] }),
  );
  await h.call("blocked again", fetchResource);
  await chrome.declarativeNetRequest.updateDynamicRules({
    removeRuleIds: [11],
  });
}
