export async function run(h) {
  for(const path of [undefined,"","callback","/callback","//callback","../callback","callback?x=1#fragment","https://elsewhere.test/","sp ace/ą"]) {
    const value=path===undefined?chrome.identity.getRedirectURL():chrome.identity.getRedirectURL(path);
    h.record(path===undefined?"default":path,value.replace(chrome.runtime.id,"<extension>"));
  }
  await h.call("invalid path",()=>chrome.identity.getRedirectURL(7));
}
