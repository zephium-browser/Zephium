function rule(id) {
  return {
    id,
    priority: 1,
    action: { type: "block" },
    condition: {
      urlFilter: "||localhost/never-requested",
      resourceTypes: ["xmlhttprequest"],
    },
  };
}

export async function run(h) {
  await h.call("initial dynamic rules", () =>
    chrome.declarativeNetRequest.getDynamicRules(),
  );
  await h.call("add dynamic rule", () =>
    chrome.declarativeNetRequest.updateDynamicRules({ addRules: [rule(1)] }),
  );
  const dynamic = await h.call("dynamic rule round trip", () =>
    chrome.declarativeNetRequest.getDynamicRules(),
  );
  h.assert(
    dynamic?.some((value) => value.id === 1),
    "The dynamic rule was not installed.",
  );
  await h.call("duplicate IDs reject atomically", () =>
    chrome.declarativeNetRequest.updateDynamicRules({
      addRules: [rule(2), rule(2)],
    }),
  );
  await h.call("dynamic rules after rejection", () =>
    chrome.declarativeNetRequest.getDynamicRules(),
  );
  await h.call("add session rule", () =>
    chrome.declarativeNetRequest.updateSessionRules({ addRules: [rule(3)] }),
  );
  const session = await h.call("session rule round trip", () =>
    chrome.declarativeNetRequest.getSessionRules(),
  );
  h.assert(
    session?.some((value) => value.id === 3),
    "The session rule was not installed.",
  );
  await h.call("remove dynamic rule", () =>
    chrome.declarativeNetRequest.updateDynamicRules({ removeRuleIds: [1] }),
  );
  await h.call("remove session rule", () =>
    chrome.declarativeNetRequest.updateSessionRules({ removeRuleIds: [3] }),
  );
  await h.call("final dynamic rules", () =>
    chrome.declarativeNetRequest.getDynamicRules(),
  );
  await h.call("final session rules", () =>
    chrome.declarativeNetRequest.getSessionRules(),
  );
}
