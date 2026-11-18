export async function run(h) {
  for (const name of [
    "@@ui_locale",
    "@@bidi_dir",
    "@@bidi_reversed_dir",
    "@@bidi_start_edge",
    "@@bidi_end_edge",
  ]) {
    await h.call(name, () => chrome.i18n.getMessage(name));
  }
  h.record("@@extension_id", {
    matchesRuntimeId:
      chrome.i18n.getMessage("@@extension_id") === chrome.runtime.id,
  });
  await h.call("getUILanguage", () => chrome.i18n.getUILanguage());
  await h.callback("getAcceptLanguages.callback", (done) =>
    chrome.i18n.getAcceptLanguages(done),
  );
  await h.call("getAcceptLanguages.promise", () =>
    chrome.i18n.getAcceptLanguages(),
  );
  const manifest = chrome.runtime.getManifest();
  h.record("manifest.localized", {
    name: manifest.name,
    description: manifest.description,
    defaultLocale: manifest.default_locale,
  });
}
