// Records every browser-data event the worker hears, so a page can check
// that events wake and reach the worker.
const seen = [];
const record = (name) => (...args) => {
  seen.push({ name, args });
  chrome.storage.local.set({ seen });
};
for (const name of ["onVisited", "onVisitRemoved"]) chrome.history[name].addListener(record(`history.${name}`));
for (const name of ["onCreated", "onChanged", "onMoved", "onRemoved"]) chrome.bookmarks[name].addListener(record(`bookmarks.${name}`));
chrome.sessions.onChanged.addListener(record("sessions.onChanged"));
for (const name of ["onCreated", "onChanged", "onErased"]) chrome.downloads[name].addListener(record(`downloads.${name}`));
