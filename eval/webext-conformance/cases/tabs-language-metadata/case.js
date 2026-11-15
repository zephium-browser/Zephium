export async function run(h) {
  for (const hint of ['none', 'fr', 'de', 'pl', 'en_us', 'zh_tw', 'invalid']) {
    const tab = await h.openTab(`${h.origins.a}/language.html?hint=${hint}`);
    await h.sleep(300);
    await h.call(hint, () => chrome.tabs.detectLanguage(tab.id));
    await chrome.tabs.remove(tab.id);
  }
}
