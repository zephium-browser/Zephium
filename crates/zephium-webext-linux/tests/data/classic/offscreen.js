chrome.runtime.onMessage.addListener((message, _, sendResponse) => {
  if (message && message.type === 'offscreenPing') {
    sendResponse({ offscreen: true, window: typeof window, copied: document.execCommand('copy') });
  }
});
