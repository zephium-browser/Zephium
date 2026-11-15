globalThis.bootstrapRuns=(globalThis.bootstrapRuns||0)+1;
chrome.runtime.sendMessage({value:globalThis.bootstrapRuns});
