chrome.runtime.onConnect.addListener((port) => {
  port.onMessage.addListener((message) => {
    if (message.op === "peer-close") port.disconnect();
    else port.postMessage({ echo: message, name: port.name });
  });
  port.onDisconnect.addListener(() => {
    chrome.runtime.sendMessage({
      probe: "remote-disconnect",
      name: port.name,
      lastError: chrome.runtime.lastError?.message ?? null,
    });
  });
  port.postMessage({ ready: true, name: port.name });
});
