export async function run(h) {
  h.record("extension catalog",chrome.i18n.getMessage("message3999"));
  const tab=await h.openTab(`${h.origins.a}/page.html`);
  await h.call("content catalog ready at document start",()=>chrome.scripting.executeScript({target:{tabId:tab.id},func:()=>globalThis.largeCatalog}));
}
