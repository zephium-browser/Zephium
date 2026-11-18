export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/frames.html`);
  try {
    const frames = await chrome.webNavigation.getAllFrames({ tabId: tab.id });
    frames.sort((a, b) => a.frameId - b.frameId);
    h.record(
      "frames",
      frames.map(({ frameId, parentFrameId, documentId, url }) => ({
        frameId,
        parentFrameId,
        documentId,
        url,
      })),
    );
    for (const frame of frames) {
      await h.call(
        `frame ${frame.frameId === 0 ? "top" : new URL(frame.url, h.origins.a).pathname}`,
        () =>
          chrome.tabs.sendMessage(
            tab.id,
            { probe: "frame-target" },
            { frameId: frame.frameId },
          ),
      );
    }
    const child = frames.find(
      (frame) => frame.parentFrameId === 0 && frame.url.startsWith(h.origins.a),
    );
    h.assert(
      child?.documentId,
      "The fixture did not expose a child frame document identifier.",
    );
    await h.call("matching frame and document", () =>
      chrome.tabs.sendMessage(
        tab.id,
        { probe: "frame-target" },
        { frameId: child.frameId, documentId: child.documentId },
      ),
    );
    await h.call("mismatched frame and document", () =>
      chrome.tabs.sendMessage(
        tab.id,
        { probe: "frame-target" },
        { frameId: 0, documentId: child.documentId },
      ),
    );
    await h.call("missing frame", () =>
      chrome.tabs.sendMessage(
        tab.id,
        { probe: "frame-target" },
        { frameId: 99999 },
      ),
    );
  } finally {
    await chrome.tabs.remove(tab.id);
  }
}
