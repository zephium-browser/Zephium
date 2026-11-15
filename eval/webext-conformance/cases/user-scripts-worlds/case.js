export async function run(h) {
  await chrome.userScripts.configureWorld({messaging: true});
  await chrome.userScripts.configureWorld({worldId: "muted", messaging: false});
  const matches = ["http://127.0.0.1/*"];
  await h.call("register", () => chrome.userScripts.register([
    {id:"default", matches, runAt:"document_start", js:[{code:`
      globalThis.userMarker='default';
      document.documentElement.dataset.userOwn = String(chrome.runtime.id.length === 32);
      document.documentElement.dataset.userSurface = JSON.stringify(Object.keys(chrome.runtime).sort());
      chrome.runtime.sendMessage({kind:'user'}).then(value => document.documentElement.dataset.userReply=JSON.stringify(value));
      chrome.runtime.onMessage.addListener((message,sender,reply)=>reply('from-user'));
      const port=chrome.runtime.connect({name:'user'});
      port.onMessage.addListener(value=>{document.documentElement.dataset.userPort=String(value);port.disconnect();});
      port.postMessage(41);
    `}]},
    {id:"muted", matches, worldId:"muted", runAt:"document_end", js:[{code:`
      document.documentElement.dataset.muted=JSON.stringify({other:typeof userMarker,send:typeof chrome.runtime.sendMessage,connect:typeof chrome.runtime.connect,onMessage:typeof chrome.runtime.onMessage,onConnect:typeof chrome.runtime.onConnect,getURL:typeof chrome.runtime.getURL,id:chrome.runtime.id.length===32,guid:/^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/.test(chrome.runtime.dynamicId)});
    `}]},
    {id:"main", matches, world:"MAIN", runAt:"document_idle", js:[{code:"globalThis.mainUserMarker=42;document.documentElement.dataset.mainUser=String(globalThis.chrome?.runtime===undefined);"}]},
  ]));
  await h.call("get scripts", () => chrome.userScripts.getScripts());
  const tab = await h.openTab(`${h.origins.a}/page.html?users`);
  let state;
  for (let i=0;i<150;i++) {
    state=(await chrome.scripting.executeScript({target:{tabId:tab.id},func:()=>({data:{...document.documentElement.dataset},isolated:typeof userMarker,main:typeof mainUserMarker})}))[0].result;
    if(state.data.userReply && state.data.userPort && state.data.muted && state.data.mainUser) break;
    await h.sleep(20);
  }
  h.record("worlds", {own:state.data.userOwn,surface:JSON.parse(state.data.userSurface||"null"),reply:JSON.parse(state.data.userReply||"null"),port:state.data.userPort,muted:JSON.parse(state.data.muted||"null"),main:state.data.mainUser,isolated:state.isolated,mainHidden:state.main});
  await h.call("tab to user", () => chrome.tabs.sendMessage(tab.id, "hello"));
  await h.call("update", () => chrome.userScripts.update([{id:"main",js:[{code:"globalThis.mainUserMarker=43;"}]}]));
  await h.call("unregister", () => chrome.userScripts.unregister({ids:["main","muted","default"]}));
  await h.call("empty", () => chrome.userScripts.getScripts());
}
