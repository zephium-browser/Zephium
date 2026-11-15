export async function run(h) {
 const result=await chrome.runtime.sendMessage("relative navigation");
 h.record("package-root navigation",JSON.parse(JSON.stringify(result).replaceAll(chrome.runtime.id,"<extension>")));
}
