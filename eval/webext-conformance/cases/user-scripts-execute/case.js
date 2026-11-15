export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  const target = {tabId:tab.id};
  const execute = (js, options={}) => chrome.userScripts.execute({target, js, ...options});
  const record = async (name, js, options={}) => h.record(name,(await execute(js,options)).map(row=>({frameId:row.frameId,result:row.result})));
  await record('default isolated', [{code:'globalThis.userDefault=41;[typeof chrome.runtime.getURL,typeof chrome.runtime.sendMessage,typeof window.pageUser]'}]);
  h.record('content isolated',(await chrome.scripting.executeScript({target,func:()=>typeof globalThis.userDefault}))[0].result);
  await record('MAIN', [{code:'globalThis.pageUser=42;typeof globalThis.userDefault'}], {world:'MAIN'});
  await record('new world one', [{code:'globalThis.customUser=43;typeof globalThis.userDefault'}],{worldId:'one'});
  await record('new world two', [{code:'typeof globalThis.customUser'}],{worldId:'two'});
  await record('world one retained', [{code:'globalThis.customUser'}],{worldId:'one'});
  await record('interleaved', [{code:"globalThis.userOrder=['code'];let userCodeLexical=22;"},{file:'part.js'},{code:"userOrder.push('last');[userOrder,userCodeLexical+userFileLexical]"}]);
  await record('promise resolved by later source', [{code:'new Promise(resolve=>{globalThis.resolveUser=resolve})'},{code:'resolveUser(42);43'}]);
  await record('earlier throw', [{code:"throw new Error('earlier')"},{code:'44'}]);
  await record('earlier rejected promise', [{code:"Promise.reject(new Error('earlier'))"},{code:'45'}]);
  await record('last throw', [{code:'46'},{code:"throw new Error('last')"}]);
  await record('last rejected promise', [{code:'47'},{code:"Promise.reject(new Error('last'))"}]);
  await record('settled then getter read once', [{code:"(()=>{let n=0;const o={};Object.defineProperty(o,'then',{get(){return ++n===1?undefined:()=>{throw 1}},enumerable:false});return o})()"}]);
  await record('last undefined', [{code:'48'},{code:'void 0'}]);
  await h.call('missing last file',()=>execute([{code:'globalThis.missingFileExecuted=true'},{file:'missing.js'}]));
  await record('missing file atomic', [{code:'typeof globalThis.missingFileExecuted'}]);
  await h.call('duplicate file',()=>execute([{file:'part.js'},{file:'part.js'}]));
  await h.call('MAIN worldId',()=>execute([{code:'1'}],{world:'MAIN',worldId:'one'}));
  await h.call('registrations remain empty',()=>chrome.userScripts.getScripts());
  await h.call('configurations remain empty',()=>chrome.userScripts.getWorldConfigurations());
  await chrome.userScripts.configureWorld({worldId:'messaged',messaging:true});
  await record('programmatic parent messaging', [{code:'chrome.runtime.sendMessage({probe:42})'}], {worldId:'messaged'});
  await chrome.userScripts.configureWorld({messaging:true});
  await record('existing muted world remains muted', [{code:'typeof chrome.runtime.sendMessage'}]);
  const fresh = await h.openTab(`${h.origins.a}/page.html?fresh-user-world`);
  const message = await chrome.userScripts.execute({target:{tabId:fresh.id},js:[{code:'chrome.runtime.sendMessage({probe:43})'}]});
  h.record('new document messaging',message.map(row=>({frameId:row.frameId,result:row.result})));

}
