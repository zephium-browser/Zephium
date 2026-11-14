export async function run(h) {
  const reports = [];
  let finish;
  const received = new Promise((resolve) => {
    finish = resolve;
  });
  const listener = (message, sender, respond) => {
    if (message.probe !== "frame-sender") return;
    reports.push({
      source: message.source,
      sender: {
        id: sender.id ?? null,
        frameId: sender.frameId ?? null,
        documentId: sender.documentId ?? null,
        url: sender.url ?? null,
        origin: sender.origin ?? null,
        tab: sender.tab
          ? {
              id: sender.tab.id,
              index: sender.tab.index,
              windowId: sender.tab.windowId,
              url: sender.tab.url ?? null,
              incognito: sender.tab.incognito,
            }
          : null,
      },
    });
    respond({ received: true });
    if (reports.length === 5) finish();
  };
  chrome.runtime.onMessage.addListener(listener);
  const tab = await h.openTab(`${h.origins.a}/frames.html`);
  try {
    await Promise.race([
      received,
      h.sleep(8000).then(() => {
        throw new Error(
          `Expected five frame senders, received ${reports.length}`,
        );
      }),
    ]);
    reports.sort((a, b) =>
      JSON.stringify(a.source).localeCompare(JSON.stringify(b.source)),
    );
    for (const report of reports) h.record("message.sender", report);
    h.assert(
      reports.filter((report) => report.source.top).length === 1,
      "Exactly one sender is the top frame",
    );
  } finally {
    chrome.runtime.onMessage.removeListener(listener);
    await chrome.tabs.remove(tab.id);
  }
}
