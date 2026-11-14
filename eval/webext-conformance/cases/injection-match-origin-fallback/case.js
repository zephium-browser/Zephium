export async function run(h) {
  const kinds = ["top", "same", "cross", "blank", "srcdoc", "data", "blob"];
  const messages = kinds.map((kind) =>
    h.event(
      chrome.runtime.onMessage,
      (message) =>
        message?.probe === "origin-fallback" && message.kind === kind,
    ),
  );
  const tab = await h.openTab(`${h.origins.a}/frames.html`);
  await h.call("create related data and blob frames", () =>
    chrome.scripting.executeScript({
      target: { tabId: tab.id },
      world: "MAIN",
      func: async () => {
        const urls = [
          "data:text/html,<html><body>data child</body></html>",
          URL.createObjectURL(
            new Blob(["<html><body>blob child</body></html>"], {
              type: "text/html",
            }),
          ),
        ];
        await Promise.all(
          urls.map(
            (url) =>
              new Promise((resolve) => {
                const frame = document.createElement("iframe");
                frame.src = url;
                frame.onload = () => resolve();
                document.body.append(frame);
              }),
          ),
        );
        return { created: urls.length };
      },
    }),
  );
  const rows = await Promise.all(messages);
  h.record(
    "origin fallback content messages",
    rows
      .map(([message]) => message)
      .sort((a, b) => a.kind.localeCompare(b.kind)),
  );
}
