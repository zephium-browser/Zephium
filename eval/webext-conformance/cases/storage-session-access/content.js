chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (message.probe !== "session-access") return;
  (async () => {
    const result = {
      localPresent: chrome.storage.local !== undefined,
      sessionPresent: chrome.storage.session !== undefined,
    };
    try {
      result.value = await chrome.storage.session.get("sessionProbe");
    } catch (error) {
      result.error = error.message;
    }
    respond(result);
  })();
  return true;
});
