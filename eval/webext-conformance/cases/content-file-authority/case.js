export async function run(h) {
  const tab = await h.openTab(h.origins.a + "/page.html");
  await h.call("selection", () => chrome.scripting.executeScript({target:{tabId:tab.id},func:()=>({valid:globalThis.validMatched===true,file:globalThis.fileMatched===true})}));
}
