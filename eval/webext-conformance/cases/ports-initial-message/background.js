chrome.runtime.onConnect.addListener(port => {
  port.postMessage({ ready: true, name: port.name });
  port.onMessage.addListener(message => port.postMessage({ echo: message }));
});
