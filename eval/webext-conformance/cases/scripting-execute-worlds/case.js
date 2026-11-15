export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  await h.call("isolated world sees DOM", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      func: () => {
        globalThis.worldProbe = "isolated";
        document.documentElement.dataset.worldProbe = "shared-dom";
        return {
          pageGlobal: typeof window.pageScriptRan,
          domMarker: document.documentElement.dataset.pageScript,
          ownGlobal: globalThis.worldProbe,
        };
      },
    }),
  );
  await h.call("main world sees page globals", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      world: "MAIN",
      func: () => {
        const priorProbe = typeof globalThis.worldProbe;
        globalThis.worldProbe = "main";
        return {
          pageGlobal: window.pageScriptRan,
          domMarker: document.documentElement.dataset.worldProbe,
          priorProbe,
          ownGlobal: globalThis.worldProbe,
          runtime: typeof globalThis.chrome?.runtime?.id,
        };
      },
    }),
  );
  await h.call("isolated global survived main write", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      world: "ISOLATED",
      func: () => globalThis.worldProbe,
    }),
  );
}
