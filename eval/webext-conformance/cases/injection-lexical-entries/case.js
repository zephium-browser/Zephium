export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  await h.call("lexical binding shared across entries", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => ({
        values: sharedLexicalEntries.values,
        windowProperty: Object.hasOwn(window, "sharedLexicalEntries"),
      }),
    }),
  );
  await h.call("later file injection", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      files: ["later.js"],
    }),
  );
  await h.call("lexical binding seen after later injection", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => sharedLexicalEntries.values,
    }),
  );
}
