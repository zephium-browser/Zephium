export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  const target = {tabId:tab.id};
  await h.call("manifest count", () => chrome.scripting.executeScript({target,func:()=>globalThis.repeatCount}));
  await chrome.scripting.executeScript({target,files:["repeat.js"]});
  await chrome.scripting.executeScript({target,files:["repeat.js"]});
  await h.call("repeat count", () => chrome.scripting.executeScript({target,func:()=>globalThis.repeatCount}));
}
