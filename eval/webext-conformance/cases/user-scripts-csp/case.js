export async function run(h) {
  await chrome.userScripts.register([{id:"eval",matches:["http://127.0.0.1/*"],runAt:"document_end",js:[{code:"try{document.documentElement.dataset.userEval=String(eval('40+2'))}catch(error){document.documentElement.dataset.userEval=error.name}"}]}]);
  const read = async (suffix) => {
    const tab=await h.openTab(`${h.origins.a}/page.html?${suffix}`);
    return (await chrome.scripting.executeScript({target:{tabId:tab.id},func:()=>document.documentElement.dataset.userEval}))[0].result;
  };
  h.record("default eval",await read("default"));
  await h.call("restrictive CSP",()=>chrome.userScripts.configureWorld({csp:"script-src 'none'"}));
  await h.call("empty CSP",()=>chrome.userScripts.configureWorld({csp:""}));
  await h.call("configuration",()=>chrome.userScripts.getWorldConfigurations());
  h.record("empty-policy eval",await read("empty"));
  await h.call("reset",()=>chrome.userScripts.resetWorldConfiguration());
  await h.call("reset configuration",()=>chrome.userScripts.getWorldConfigurations());
}
