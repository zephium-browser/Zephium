export async function run(h) {
  h.record("uiLanguage", chrome.i18n.getUILanguage());
  await h.call("regional.override", () => chrome.i18n.getMessage("shared"));
  await h.call("regional.only", () => chrome.i18n.getMessage("regional_only"));
  await h.call("default.fallback", () =>
    chrome.i18n.getMessage("default_only"),
  );
  await h.call("default.placeholderFallback", () =>
    chrome.i18n.getMessage("default_placeholder", "Ada"),
  );
  await h.call("missing.everyCatalog", () =>
    chrome.i18n.getMessage("missing_everywhere"),
  );
}
