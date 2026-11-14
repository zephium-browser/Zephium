export async function run(h) {
  h.record("sendMessage.call", { mode: "callback", checksLastError: false });
  await new Promise((resolve) => {
    chrome.runtime.sendMessage(
      { probe: "unchecked-no-receiver" },
      (...args) => {
        h.record("sendMessage.callback", { args });
        resolve();
      },
    );
  });
  await h.sleep(0);
  h.record("lastError.afterTask", {
    present: chrome.runtime.lastError !== undefined,
  });
  const messages = await h.host("consoleMessages", {});
  const diagnostics = [
    ...new Set(
      messages
        .map((entry) => (typeof entry === "string" ? entry : entry.text))
        .filter((text) => /Unchecked runtime\.lastError/.test(text)),
    ),
  ];
  h.assert(
    diagnostics.length > 0,
    "The unchecked error produced a console diagnostic",
  );
  h.record("console.uncheckedLastError", diagnostics);
}
