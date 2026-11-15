let queue = Promise.resolve();
chrome.runtime.onMessage.addListener((message, sender, respond) => {
  queue = queue.then(async () => {
    if (message.kind === 'hello') {
      const { seen = 0, popup = false } = await chrome.storage.local.get(['seen', 'popup']);
      await chrome.storage.local.set({ seen: seen + 1 });
      const tabs = await chrome.tabs.query({});
      respond({ seen: seen + 1, popup, tabs: tabs.length, url: tabs[0]?.url ?? null, tab: sender.tab?.id ?? null, frame: sender.frameId ?? null });
    } else if (message.kind === 'popup') {
      await chrome.storage.local.set({ popup: true });
      const [tab] = await chrome.tabs.query({ active: true });
      respond({ popup: true, tab: tab?.id ?? null });
    }
  }, () => {});
  return true;
});
