return JSON.stringify({
  enabled: await chrome.declarativeNetRequest.getEnabledRulesets(),
  mode: await chrome.storage.local.get('filteringModeDetails'),
});
