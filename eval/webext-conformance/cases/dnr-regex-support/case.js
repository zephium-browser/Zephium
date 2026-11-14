export async function run(h) {
  await h.call("valid regex", () =>
    chrome.declarativeNetRequest.isRegexSupported({
      regex: "^http://localhost/.*$",
    }),
  );
  await h.call("invalid regex", () =>
    chrome.declarativeNetRequest.isRegexSupported({ regex: "[" }),
  );
  await h.call("backreference regex", () =>
    chrome.declarativeNetRequest.isRegexSupported({ regex: "(a)\\1" }),
  );
  await h.call("capture required", () =>
    chrome.declarativeNetRequest.isRegexSupported({
      regex: "(localhost)",
      isCaseSensitive: true,
      requireCapturing: true,
    }),
  );
  await h.callback("regex callback", (callback) =>
    chrome.declarativeNetRequest.isRegexSupported(
      { regex: "localhost" },
      callback,
    ),
  );
}
