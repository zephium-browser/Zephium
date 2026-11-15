(globalThis.injectionStages ??= []).push({
  stage: "idle",
  parsed: document.readyState !== "loading",
  pageGlobal: typeof window.pageScriptRan,
  pageMarker: document.documentElement?.dataset.pageScript ?? null,
});
chrome.runtime.sendMessage({ probe: "run-at-idle" });
