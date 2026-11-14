import { helperValue } from './lib/helper.mjs';

const awaited = await new Promise((resolve) => setTimeout(() => resolve('top-level await'), 10));
let importScriptsThrows = false;
try {
  importScripts('lib/helper.js');
} catch {
  importScriptsThrows = true;
}
const scope = {
  serviceWorker: self instanceof ServiceWorkerGlobalScope,
  window: typeof window,
  document: typeof document,
  helper: helperValue,
  module: true,
  awaited,
  importScriptsThrows,
  greeting: chrome.i18n.getMessage('greeting', ['now']),
};

chrome.runtime.onInstalled.addListener(async (details) => {
  const { installs = 0 } = await chrome.storage.local.get('installs');
  await chrome.storage.local.set({ installs: installs + 1, lastReason: details.reason });
});

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  if (message && message.type === 'probe') {
    sendResponse({ ...scope, sender: sender.url, id: chrome.runtime.id });
  } else if (message && message.type === 'storage') {
    chrome.storage.local.get(null).then(sendResponse);
    return true;
  }
  return false;
});
