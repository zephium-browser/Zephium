export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  await h.call("sync function with JSON arguments", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      args: ["hello", { count: 3, values: [true, null, "x"] }],
      func: (text, value) => ({
        text,
        count: value.count + 1,
        values: value.values,
        top: self === top,
      }),
    }),
  );
  await h.call("promise result is awaited", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      args: [7],
      func: async (value) => {
        await Promise.resolve();
        return { value: value * 2, awaited: true };
      },
    }),
  );
  await h.call("undefined result shape", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => undefined,
    }),
  );
  await h.callback("executeScript callback", (callback) =>
    chrome.scripting.executeScript(
      { target: { tabId: tab.id }, func: () => "callback" },
      callback,
    ),
  );
}
