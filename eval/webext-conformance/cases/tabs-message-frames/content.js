chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message.probe === "frame-target")
    sendResponse({
      url: location.href,
      name: window.name,
      top: window === top,
    });
});
