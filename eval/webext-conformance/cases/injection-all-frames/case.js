export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/frames.html`);
  await h.call("frame injection markers", async () => {
    const rows = await chrome.scripting.executeScript({
      target: { tabId: tab.id, allFrames: true },
      func: () => ({
        url: location.href,
        name: window.name,
        top: self === top,
        topOnly: document.documentElement.dataset.topOnly ?? null,
        allFrames: document.documentElement.dataset.allFrames ?? null,
      }),
    });
    return rows.sort((a, b) =>
      `${a.result.top ? "0" : "1"}:${a.result.url}:${a.result.name}`.localeCompare(
        `${b.result.top ? "0" : "1"}:${b.result.url}:${b.result.name}`,
      ),
    );
  });
}
