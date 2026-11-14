chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  const response = { style: message.style, echo: message.value };
  if (message.style === "synchronous") {
    sendResponse(response);
    return false;
  }
  if (message.style === "deferred") {
    setTimeout(() => sendResponse(response), 0);
    return true;
  }
  if (message.style === "promise") return Promise.resolve(response);
  return false;
});
