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
  ];
  const listeners = names.map((name) => {
    const listener = (details) => {
      if (details.tabId === tab.id) rows.push(project(name, details));
    };
    chrome.webNavigation[name].addListener(listener);
    return listener;
  });
  try {
    const done = h.event(
      chrome.webNavigation.onCompleted,
      (details) => details.tabId === tab.id && details.frameId === 0,
    );
    await chrome.tabs.update(tab.id, { url: `${h.origins.a}/frames.html` });
    await done;
    const frames = await chrome.webNavigation.getAllFrames({ tabId: tab.id });
    frames.sort((a, b) =>
      `${a.frameId === 0 ? "0" : "1"}:${a.url}`.localeCompare(
        `${b.frameId === 0 ? "0" : "1"}:${b.url}`,
      ),
    );
    h.record("frames", frames);
    h.record(
      "per-frame event sequences",
      frames.map((frame) => ({
        frameId: frame.frameId,
        url: frame.url,
        events: rows.filter((row) => row.frameId === frame.frameId),
      })),
    );
    h.record(
      "parent committed before child committed",
      frames
        .filter((frame) => frame.parentFrameId >= 0)
        .every(
          (frame) =>
            rows.findIndex(
              (row) =>
                row.event === "onCommitted" &&
                row.frameId === frame.parentFrameId,
            ) <
            rows.findIndex(
              (row) =>
                row.event === "onCommitted" && row.frameId === frame.frameId,
            ),
        ),
    );
  } finally {
    names.forEach((name, index) =>
      chrome.webNavigation[name].removeListener(listeners[index]),
    );
  }
}
