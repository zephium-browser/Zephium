export async function run(h) {
  await h.call("create parser",()=>chrome.offscreen.createDocument({url:"offscreen.html",reasons:["DOM_PARSER"],justification:"Parse owned inert HTML"}));
  await h.call("parsed result",()=>chrome.runtime.sendMessage({probe:"dom-parser"}));
  await new Promise(resolve=>setTimeout(resolve, 35000));
  await h.call("parser survives idle",()=>chrome.offscreen.hasDocument());
  await h.call("parsed after idle",()=>chrome.runtime.sendMessage({probe:"dom-parser"}));
  await h.call("close parser",()=>chrome.offscreen.closeDocument());
  await h.call("parser closed",()=>chrome.offscreen.hasDocument());
}
