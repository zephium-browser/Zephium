export async function run(h) {
  const tab=await h.openTab(`${h.origins.a}/page.html`);
  const event=chrome.webRequest.onBeforeRequest;
  const filters=[
    ["own extension",{urls:[chrome.runtime.getURL("*.html*")]}],
    ["file authority",{urls:["file://*/*.user.js","file://*/*.user.js?*"]}],
    ["socket schemes",{urls:["ws://*/*","wss://*/*"]}],
    ["empty lists",{urls:[],types:[],tabId:-1,windowId:-1}],
  ];
  const listeners=[];let observed=false;
  for(const [name,filter] of filters){
    const listener=d=>{if(name==="empty lists"&&d.url.endsWith("/resource.txt"))observed=true};
    event.addListener(listener,filter);listeners.push(listener);
    h.record(name,{registered:event.hasListener(listener)});
  }
  await h.sleep(100);
  await chrome.scripting.executeScript({target:{tabId:tab.id},func:()=>fetch("/resource.txt").then(r=>r.text())});
  for(let i=0;i<50&&!observed;i++)await h.sleep(20);
  h.record("unrestricted request",observed);
  for(const listener of listeners)event.removeListener(listener);
}
