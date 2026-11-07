const started = Date.now();
chrome.storage.local.get("boots").then(({ boots = [] }) => chrome.storage.local.set({ boots: [...boots, started] }));
chrome.privacy.services.passwordSavingEnabled.onChange.addListener((details) => {
  chrome.storage.local.set({ changed: { value: details.value, level: details.levelOfControl, started } });
});
chrome.idle.onStateChanged.addListener((state) => chrome.storage.local.set({ idle: state }));
