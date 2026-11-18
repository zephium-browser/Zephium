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
}
