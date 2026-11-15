export async function run(h) {
  await h.call("worker self messages", () => chrome.runtime.sendMessage({report:true}));
}
