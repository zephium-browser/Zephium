export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  await h.call("lexical binding shared across files", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => ({
        count: sharedLexicalFiles.count,
        strictThisUndefined: globalThis.strictThisUndefined,
        windowProperty: Object.hasOwn(window, "sharedLexicalFiles"),
      }),
    }),
  );
}
