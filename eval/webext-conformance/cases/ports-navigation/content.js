chrome.runtime.onConnect.addListener((port) => {
  port.onMessage.addListener((message) => port.postMessage({ echo: message }));
  port.postMessage({ ready: true, name: port.name });
});
