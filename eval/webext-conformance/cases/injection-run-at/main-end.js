(globalThis.injectionStages ??= []).push({
  stage: "end",
  parsed: document.readyState !== "loading",
  pageGlobal: typeof window.pageScriptRan,
  pageMarker: document.documentElement?.dataset.pageScript ?? null,
});
