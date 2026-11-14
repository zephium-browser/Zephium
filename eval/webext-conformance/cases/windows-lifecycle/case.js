function view(value) {
  return {
    windowId: value.id,
    type: value.type,
    incognito: value.incognito,
    state: value.state,
    tabs: value.tabs?.map((tab) => ({
      id: tab.id,
      windowId: tab.windowId,
      active: tab.active,
      index: tab.index,
    })),
  };
}

export async function run(h) {
  await h.call("current window", async () =>
    view(await chrome.windows.getCurrent({ populate: true })),
  );
  const created = await h.call("create normal window", async () =>
    view(
      await chrome.windows.create({
        url: "about:blank",
        focused: false,
        type: "normal",
      }),
    ),
  );
  h.assert(
    created,
    "Window creation failed; window lifecycle was not exercised.",
  );
  try {
    await h.call("get populated window", async () =>
      view(await chrome.windows.get(created.windowId, { populate: true })),
    );
    await h.call("update window", async () =>
      view(await chrome.windows.update(created.windowId, { focused: false })),
    );
    const removed = h.event(
      chrome.windows.onRemoved,
      (id) => id === created.windowId,
    );
    await h.call("remove window", () =>
      chrome.windows.remove(created.windowId),
    );
    h.record("removed event", { windowId: (await removed)[0] });
    await h.call("get removed window", () =>
      chrome.windows.get(created.windowId),
    );
  } catch (error) {
    await chrome.windows.remove(created.windowId).catch(() => {});
    throw error;
  }
}
