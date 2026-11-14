function project(event, details) {
  const {
    tabId,
    frameId,
    parentFrameId,
    url,
    documentId,
    documentLifecycle,
    frameType,
    transitionType,
    transitionQualifiers,
    error,
  } = details;
  return {
    event,
    tabId,
    frameId,
    parentFrameId,
    url,
    documentId,
    documentLifecycle,
    frameType,
    transitionType,
    transitionQualifiers,
    error,
  };
}

export async function run(h) {
  const tab = await h.openTab("about:blank");
  const rows = [];
  const names = [
    "onBeforeNavigate",
    "onCommitted",
    "onDOMContentLoaded",
    "onCompleted",
    "onErrorOccurred",
  ];
  const listeners = names.map((name) => {
    const listener = (details) => {
      if (details.tabId === tab.id && details.frameId === 0)
        rows.push(project(name, details));
    };
    chrome.webNavigation[name].addListener(listener);
    return listener;
  });
  try {
    const done = h.event(
      chrome.webNavigation.onCompleted,
      (details) => details.tabId === tab.id && details.frameId === 0,
    );
    await h.call("navigate", async () => {
      await chrome.tabs.update(tab.id, {
        url: `${h.origins.a}/page.html?navigation`,
      });
      return { requested: true };
    });
    await done;
    h.record("navigation events", rows);
  } finally {
    names.forEach((name, index) =>
      chrome.webNavigation[name].removeListener(listeners[index]),
    );
  }
}
