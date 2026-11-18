export async function run(h) {
  await h.call("get matched rules without feedback", () =>
    chrome.declarativeNetRequest.getMatchedRules({}),
  );
  await h.callback("get matched rules callback without feedback", (callback) =>
    chrome.declarativeNetRequest.getMatchedRules({}, callback),
  );
}
