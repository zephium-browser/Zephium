export async function run(h) {
  h.record("extension origin", {
    origin: location.origin,
    protocol: location.protocol,
    isSecureContext,
    runtimeOrigin: new URL(chrome.runtime.getURL("/")).origin,
    windowType: typeof window,
    navigatorStorage: typeof navigator.storage,
  });
  const violations = [];
  const violation = (event) =>
    violations.push({
      blockedURI: event.blockedURI,
      effectiveDirective: event.effectiveDirective,
      violatedDirective: event.violatedDirective,
      disposition: event.disposition,
    });
  document.addEventListener("securitypolicyviolation", violation);
  try {
    const script = document.createElement("script");
    script.textContent = "globalThis.conformanceInlineExecuted = true;";
    document.documentElement.append(script);
    await h.sleep(50);
    h.record("inline executed", globalThis.conformanceInlineExecuted === true);
    await h.call("eval under extension CSP", () => (0, eval)("1 + 2"));
    await h.sleep(50);
    await h.call("Function under extension CSP", () =>
      new Function("return 3")(),
    );
    await h.sleep(50);
    h.record("CSP violations", violations);
  } finally {
    document.removeEventListener("securitypolicyviolation", violation);
  }
}
