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
  for (const fragment of ["one", "two"]) {
    const pending = h.event(
      chrome.webNavigation.onReferenceFragmentUpdated,
      (details) =>
        details.tabId === tab.id &&
        details.frameId === 0 &&
        details.url.endsWith(`#${fragment}`),
    );
    await h.call(`fragment ${fragment}`, () =>
      chrome.scripting.executeScript({
        target: { tabId: tab.id },
        world: "MAIN",
        args: [fragment],
        func: (value) => {
          if (value === "one") location.hash = value;
          else location.replace(`#${value}`);
          return { requested: value };
        },
      }),
    );
    const [details] = await pending;
    h.record(
      `fragment ${fragment} event`,
      project("onReferenceFragmentUpdated", details),
    );
  }
}
