// WebView2 creates native tabs but may omit the tabs.create result. Keep native
// ownership and events; match URLs first, then FIFO when URLs are unavailable.
(() => {
  // The worker's `chrome` global can be replaced later (Proton Pass does);
  // keep the native objects seen at load time.
  const tabs = chrome.tabs;
  const runtime = chrome.runtime;
  if (!tabs?.create || !tabs.onCreated || !runtime) return;
  const create = tabs.create;
  const pending = [];
  const normalize = value => {
    if (typeof value !== 'string' || !value) return null;
    try { return new URL(value, runtime.getURL('/')).href; } catch { return null; }
  };
  const onCreated = tab => {
    if (!Number.isInteger(tab?.id)) return;
    const url = normalize(tab.pendingUrl || tab.url);
    const candidates = pending.filter(request => !request.tab);
    // A known mismatch may be a user-created tab: never return it merely
    // because it arrived first. FIFO is only for indistinguishable events.
    const request = (url && candidates.find(request => request.url === url)) ||
      candidates.find(request => !url || !request.url);
    if (request) {
      request.tab = tab;
      request.finish();
    }
  };
  tabs.create = function (properties, callback) {
    const promise = new Promise((resolve, reject) => {
      const request = {url: normalize(properties?.url), tab: null, nativeDone: false, finish: null};
      const remove = () => {
        clearTimeout(timer);
        const index = pending.indexOf(request);
        if (index !== -1) pending.splice(index, 1);
        if (!pending.length) tabs.onCreated.removeListener(onCreated);
      };
      request.finish = () => {
        if (!request.nativeDone || !request.tab) return;
        remove(); resolve(request.tab);
      };
      const timer = setTimeout(() => { remove(); reject(new Error('Native tab creation timed out')); }, 15000);
      if (!pending.length) tabs.onCreated.addListener(onCreated);
      pending.push(request);
      try {
        Reflect.apply(create, this, [properties, tab => {
          const error = runtime.lastError;
          if (error) { remove(); reject(new Error(error.message)); return; }
          request.nativeDone = true;
          if (Number.isInteger(tab?.id)) request.tab = tab;
          request.finish();
        }]);
      } catch (error) { remove(); reject(error); }
    });
    if (typeof callback !== 'function') return promise;
    promise.then(callback, error => {
      // Match the callback API's synchronous lastError lifetime.
      const descriptor = Object.getOwnPropertyDescriptor(runtime, 'lastError');
      try {
        Object.defineProperty(runtime, 'lastError', {configurable: true, value: {message: error.message}});
        callback();
      } finally {
        if (descriptor) Object.defineProperty(runtime, 'lastError', descriptor);
        else delete runtime.lastError;
      }
    });
  };
})();

// The host relays an explicit toolbar click through our own extension page.
// Native listeners stay registered so any future native action route still works.
(() => {
  const {action, tabs, runtime} = chrome;
  const event = action?.onClicked;
  if (!event || !runtime) return;
  const listeners = new Set();
  const add = event.addListener, remove = event.removeListener;
  event.addListener = function (listener) {
    Reflect.apply(add, this, [listener]); listeners.add(listener);
  };
  event.removeListener = function (listener) {
    Reflect.apply(remove, this, [listener]); listeners.delete(listener);
  };
  runtime.onMessage.addListener((message, sender, reply) => {
    if (sender.id !== runtime.id ||
        sender.url !== runtime.getURL('zephium-windows-host/host.html') ||
        message?.__zephiumActionClick !== true || !Number.isInteger(message.tabId)) return;
    (async () => {
      const tab = await tabs.get(message.tabId);
      if (!await action.isEnabled(tab.id) || await action.getPopup({tabId: tab.id}))
        throw new Error('Action changed before click');
      for (const listener of [...listeners]) {
        try { Promise.resolve(listener(tab)).catch(() => {}); } catch { /* Other listeners still run. */ }
      }
      return {dispatched: true};
    })().then(reply, error => reply({error: String(error)}));
    return true;
  });
})();
