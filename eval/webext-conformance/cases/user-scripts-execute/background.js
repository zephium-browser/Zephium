chrome.runtime.onUserScriptMessage.addListener((message, sender, reply) => {
  reply({probe:message.probe, own:sender.id === chrome.runtime.id, frame:sender.frameId});
});
