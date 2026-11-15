export async function run(h) {
  await h.call("named.array", () =>
    chrome.i18n.getMessage("welcome", ["Ada", "12"]),
  );
  await h.call("named.string", () => chrome.i18n.getMessage("welcome", "Ada"));
  await h.call("named.omitted", () => chrome.i18n.getMessage("welcome"));
  await h.call("named.caseInsensitive", () =>
    chrome.i18n.getMessage("WELCOME", ["Ada", "12"]),
  );
  await h.call("positional", () =>
    chrome.i18n.getMessage("positional", ["one", "two", "unused"]),
  );
  await h.call("nested", () => chrome.i18n.getMessage("nested", "value"));
  await h.call("dollars", () => chrome.i18n.getMessage("dollars", "$literal"));
  await h.call("missing", () => chrome.i18n.getMessage("no_such_message"));
  await h.call("invalid.missingName", () => chrome.i18n.getMessage());
}
