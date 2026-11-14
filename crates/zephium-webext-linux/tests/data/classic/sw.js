importScripts('lib/helper.js');
const startedAt = Date.now();
const scope = {
  serviceWorker: self instanceof ServiceWorkerGlobalScope,
  window: typeof window,
  document: typeof document,
  helper: self.helperValue,
  module: false,
  greeting: chrome.i18n.getMessage('greeting', ['now']),
};

chrome.runtime.onInstalled.addListener(async (details) => {
  const { installs = 0 } = await chrome.storage.local.get('installs');
  await chrome.storage.local.set({ installs: installs + 1, lastReason: details.reason });
});

chrome.alarms.onAlarm.addListener(async (alarm) => {
  const { alarmCount = 0 } = await chrome.storage.local.get('alarmCount');
  await chrome.storage.local.set({ alarmCount: alarmCount + 1, lastAlarm: alarm.name });
});

const handlers = {
  probe: async (_, sender) => ({ ...scope, startedAt, sender: sender.url, id: chrome.runtime.id }),
  storage: () => chrome.storage.local.get(null),
  alarm: async (message) => {
    await chrome.alarms.create(message.name, { delayInMinutes: message.minutes });
    return true;
  },
  offscreenOpen: () =>
    chrome.offscreen
      .createDocument({ url: 'offscreen.html', reasons: ['CLIPBOARD'], justification: 'fixture' })
      .then(() => true, (error) => String(error && error.message)),
  offscreenSecond: () =>
    chrome.offscreen
      .createDocument({ url: 'offscreen.html', reasons: ['DOM_PARSER'], justification: 'fixture' })
      .then(() => 'created', (error) => String(error && error.message)),
  offscreenHas: () => chrome.offscreen.hasDocument(),
  offscreenClose: () => chrome.offscreen.closeDocument().then(() => true, (error) => String(error && error.message)),
  fail: () => {
    console.error('fixture console error');
    Promise.reject(new Error('fixture rejection'));
    setTimeout(() => {
      throw new Error('fixture uncaught');
    }, 0);
    return true;
  },
};

chrome.runtime.onMessage.addListener((message, sender, sendResponse) => {
  const handler = handlers[message && message.type];
  if (!handler) return false;
  Promise.resolve(handler(message, sender)).then(sendResponse, (error) => sendResponse({ error: String(error) }));
  return true;
});
