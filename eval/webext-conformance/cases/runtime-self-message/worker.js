const order = ["start"];
let ownDeliveries = 0;
const settled = error => ({
  error:error.message,
  afterFirstPass:order.includes("end"),
  afterMicrotask:order.includes("microtask"),
});
const before = chrome.runtime.sendMessage({selfProbe:true}).then(
  () => ({unexpected:true}), settled,
);
let installed;
const duringInstall = new Promise(resolve => { installed = resolve; });
self.addEventListener("install", event => {
  event.waitUntil(chrome.runtime.sendMessage(chrome.runtime.id, {selfProbe:true}).then(
    () => installed({unexpected:true}), error => installed(settled(error)),
  ));
});
chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (message.selfProbe) { ownDeliveries++; respond("self"); }
  if (message.report) {
    Promise.all([before, after, duringInstall]).then(([before, after, duringInstall]) =>
      respond({before,after,duringInstall,ownDeliveries}));
    return true;
  }
});
const after = new Promise(resolve => chrome.runtime.sendMessage({selfProbe:true}, function(value) {
  resolve({
    ...settled(chrome.runtime.lastError || {}), value:value ?? null,
    argumentCount:arguments.length,
  });
}));
queueMicrotask(() => order.push("microtask"));
order.push("end");
