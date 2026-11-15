chrome.runtime.onUserScriptMessage.addListener((message, sender, reply) => {
  reply({kind: message.kind, own: sender.id === chrome.runtime.id, frame: sender.frameId});
});
chrome.runtime.onUserScriptConnect.addListener(port => {
  port.onMessage.addListener(value => port.postMessage(value + 1));
});
