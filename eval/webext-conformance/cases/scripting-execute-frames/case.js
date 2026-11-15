export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/frames.html`);
  const rows = await h.call("all frame results", async () => {
    const result = await chrome.scripting.executeScript({
      target: { tabId: tab.id, allFrames: true },
      func: () => ({
        url: location.href,
        name: window.name,
        top: self === top,
      }),
    });
    return result.sort((a, b) =>
      `${a.result.top ? "0" : "1"}:${a.result.url}:${a.result.name}`.localeCompare(
        `${b.result.top ? "0" : "1"}:${b.result.url}:${b.result.name}`,
      ),
    );
  });
  h.assert(rows?.length >= 3, "top and both HTTP child frames were injected");
  const cross = rows.find((row) => row.result.url.startsWith(h.origins.b));
  h.assert(Boolean(cross), "cross-origin frame result exists");
  await h.call("explicit cross-origin frame", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id, frameIds: [cross.frameId] },
      func: () => ({ url: location.href, top: self === top }),
    }),
  );
}
