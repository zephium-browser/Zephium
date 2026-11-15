export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  const read = () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => {
        const style = getComputedStyle(document.documentElement);
        return {
          marker: style.getPropertyValue("--webext-probe").trim(),
          color: style.color,
          sheets: document.styleSheets.length,
          adopted: document.adoptedStyleSheets.length,
        };
      },
    });
  await h.call("manifest stylesheet", read);
  const injection = {
    target: { tabId: tab.id },
    css: "html { --webext-probe: programmatic; color: rgb(21, 43, 65); }",
    origin: "AUTHOR",
  };
  await h.call("insert CSS", () => chrome.scripting.insertCSS(injection));
  await h.call("programmatic stylesheet", read);
  await h.call("remove CSS", () => chrome.scripting.removeCSS(injection));
  await h.call("manifest stylesheet restored", read);
  await h.call("page important rule", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => {
        const style = document.createElement("style");
        style.textContent = "html { color: rgb(1, 2, 3) !important; }";
        document.head.append(style);
      },
    }),
  );
  await h.call("AUTHOR important insertion", () =>
    chrome.scripting.insertCSS({
      ...injection,
      css: "html { color: rgb(9, 8, 7) !important; }",
    }),
  );
  await h.call("AUTHOR important computed style", read);
  await h.call("USER insertion", () =>
    chrome.scripting.insertCSS({
      ...injection,
      origin: "USER",
      css: "html { color: rgb(9, 8, 7) !important; }",
    }),
  );
  await h.call("USER computed style", read);
  await h.call("navigation", async () => {
    const loaded = new Promise((resolve) => {
      const listener = (id, change) => {
        if (id === tab.id && change.status === "complete") {
          chrome.tabs.onUpdated.removeListener(listener);
          resolve();
        }
      };
      chrome.tabs.onUpdated.addListener(listener);
    });
    await chrome.tabs.update(tab.id, { url: `${h.origins.a}/page.html?next` });
    await loaded;
    return true;
  });
  await h.call("new document has no programmatic CSS", read);
  const framed = await h.openTab(`${h.origins.a}/web-frames.html`);
  const all = {
    target: { tabId: framed.id, allFrames: true },
    css: "html { --all-css: yes; }",
  };
  await h.call("all-frame insertion", () => chrome.scripting.insertCSS(all));
  const readFrames = async () =>
    (
      await chrome.scripting.executeScript({
        target: all.target,
        func: () => ({
          top: self === top,
          url: location.href,
          marker: getComputedStyle(document.documentElement)
            .getPropertyValue("--all-css")
            .trim(),
          sheets: document.styleSheets.length,
          adopted: document.adoptedStyleSheets.length,
        }),
      })
    )
      .map((row) => row.result)
      .sort((a, b) => a.url.localeCompare(b.url));
  await h.call("all-frame computed style", readFrames);
  await h.call("all-frame removal", () => chrome.scripting.removeCSS(all));
  await h.call("all-frame removal computed style", readFrames);
}
