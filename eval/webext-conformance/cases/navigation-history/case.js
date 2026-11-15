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
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  for (const operation of ["pushState", "replaceState"]) {
    const pending = h.event(
      chrome.webNavigation.onHistoryStateUpdated,
      (details) => details.tabId === tab.id && details.frameId === 0,
    );
    await h.call(operation, () =>
      chrome.scripting.executeScript({
        target: { tabId: tab.id },
        world: "MAIN",
        args: [operation],
        func: (method) => {
          history[method]({ marker: method }, "", `?history=${method}`);
          return { state: history.state, url: location.href };
        },
      }),
    );
    const [details] = await pending;
    h.record(`${operation} event`, project("onHistoryStateUpdated", details));
  }
}
