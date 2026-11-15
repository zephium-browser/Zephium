chrome.action.onClicked.addListener((tab) => {
  chrome.storage.local.set({
    actionClick: { tabId: tab.id, url: tab.url, title: tab.title },
  });
});
