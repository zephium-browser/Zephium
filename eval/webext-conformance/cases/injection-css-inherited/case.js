export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/frames.html`);
  await h.call("inherited-origin insertion", () =>
    chrome.scripting.insertCSS({
      target: { tabId: tab.id, allFrames: true },
      css: "html { --inherited-css: yes; }",
    }),
  );
  await h.call("inherited-origin computed style", async () =>
    (
      await chrome.scripting.executeScript({
        target: { tabId: tab.id, allFrames: true },
        func: () => ({
          url: location.href,
          marker: getComputedStyle(document.documentElement)
            .getPropertyValue("--inherited-css")
            .trim(),
        }),
      })
    )
      .map((row) => row.result)
      .sort((a, b) => a.url.localeCompare(b.url)),
  );
}
