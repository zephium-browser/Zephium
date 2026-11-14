(globalThis.injectionStages ??= []).push({
  stage: "start",
  parsed: document.readyState !== "loading",
  pageGlobal: typeof window.pageScriptRan,
  pageMarker: document.documentElement?.dataset.pageScript ?? null,
});
