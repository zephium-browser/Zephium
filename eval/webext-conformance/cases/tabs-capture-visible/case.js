export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/frames.html`);
  await chrome.tabs.update(tab.id, { active: true });
  for (const format of ["png", "jpeg"]) {
    await h.call(`capture ${format}`, async () => {
      const image = await chrome.tabs.captureVisibleTab(tab.windowId, { format });
      return {
        mime: image.slice(0, image.indexOf(";")),
        encoded: image.includes(";base64,") && image.length > 100,
      };
    });
  }
}
