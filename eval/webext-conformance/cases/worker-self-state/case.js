export async function run(h) {
  await h.call("worker state",()=>chrome.runtime.sendMessage("state"));
}
