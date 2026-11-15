export async function run(h) {
  for(const path of ["/page.html#fragment","/page.html%23fragment.html"]) {
    const tab=await h.openTab(h.origins.a+path);
    await h.call(path,()=>chrome.scripting.executeScript({target:{tabId:tab.id},func:()=>({valid:globalThis.validMatched===true,fragment:globalThis.fragmentMatched===true})}));
  }
}
