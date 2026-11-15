export async function run(h) {
  const id=chrome.runtime.id;
  const paths=['public/data.txt','public','public/','public/%64ata.txt','caf%c3%a9.txt','public/%2Fdata.txt','public/%00data.txt','public/%23x.txt','public/%3Fx.txt','public/%252e%252e/private.txt','/public/data.txt?query=1#hash','x/../public/data.txt','public/../private.txt','static.txt','café.txt','https://foreign.example/public/data.txt','//foreign.example/public/data.txt','///icons/../popup.html'];
  for(const path of paths) {
    const url=new URL(chrome.runtime.getURL(path));
    h.record(path,{dynamic:url.hostname!==id,path:url.pathname,search:url.search,hash:url.hash});
  }
  h.record('identity stable',chrome.runtime.id===id);
  const first=chrome.runtime.getURL('public/data.txt');
  h.record('stable locator',first===chrome.runtime.getURL('public/data.txt'));
  const permitted=await h.openTab(`${h.origins.a}/page.html`);
  const denied=await h.openTab(`${h.origins.b}/page.html`);
  const content=await chrome.scripting.executeScript({target:{tabId:permitted.id},func:()=>({dynamic:new URL(chrome.runtime.getURL('public/data.txt')).hostname!==chrome.runtime.id})});
  h.record('content alias',content[0].result.dynamic);
  for(const [label,tab,url] of [['allowed alias',permitted,first],['denied alias',denied,first],['private',permitted,chrome.runtime.getURL('private.txt')]]) {
    const result=await chrome.scripting.executeScript({target:{tabId:tab.id},world:'MAIN',func:async(url)=>{try {const r=await fetch(url);return {allowed:r.ok,text:r.ok?await r.text():''};}catch{return {allowed:false,text:''};}},args:[url]});
    h.record(label,result[0].result);
  }
  await chrome.tabs.remove([permitted.id,denied.id]);
}
