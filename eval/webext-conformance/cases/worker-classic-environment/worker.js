let initialImport;
try {
  importScripts("imported.js");
  initialImport = { ok: true, value: self.conformanceImported };
} catch (error) {
  initialImport = {
    ok: false,
    error: { name: error.name, message: error.message },
  };
}

async function facts() {
  let lateImport;
  try {
    importScripts("late-import.js");
    lateImport = { ok: true, value: self.conformanceLateImported };
  } catch (error) {
    lateImport = {
      ok: false,
      error: { name: error.name, message: error.message },
    };
  }
  return {
    windowType: typeof window,
    documentType: typeof document,
    isServiceWorker: self instanceof ServiceWorkerGlobalScope,
    isSecureContext,
    origin: location.origin,
    registration: {
      scope: self.registration.scope,
      activeState: self.registration.active?.state,
      installing: Boolean(self.registration.installing),
      waiting: Boolean(self.registration.waiting),
      updateType: typeof self.registration.update,
    },
    clients: {
      matchAll: typeof clients.matchAll,
      claim: typeof clients.claim,
      openWindow: typeof clients.openWindow,
    },
    navigatorStorage: {
      type: typeof navigator.storage,
      estimate: typeof navigator.storage?.estimate,
      persisted: typeof navigator.storage?.persisted,
    },
    importScriptsType: typeof importScripts,
    initialImport,
    lateImport,
  };
}
chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  if (message.probe !== "worker-facts") return false;
  facts().then(sendResponse, (error) =>
    sendResponse({ error: { name: error.name, message: error.message } }),
  );
  return true;
});
