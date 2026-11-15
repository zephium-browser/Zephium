export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  const rows = [];
  const registrations = [];
  for (const event of [
    "onBeforeRequest",
    "onBeforeRedirect",
    "onCompleted",
    "onErrorOccurred",
  ]) {
    const listener = (details) => rows.push({ event, ...details });
    chrome.webRequest[event].addListener(listener, {
      urls: ["http://127.0.0.1/request-*", "http://127.0.0.1/resource.txt"],
      tabId: tab.id,
    });
    registrations.push([event, listener]);
  }
  await chrome.scripting.executeScript({
    target: { tabId: tab.id },
    func: async () => {
      await fetch("/request-redirect");
      try {
        await fetch("/request-fail");
      } catch {}
    },
  });
  for (
    let attempt = 0;
    attempt < 50 && !rows.some((row) => row.event === "onErrorOccurred");
    attempt++
  )
    await h.sleep(20);
  for (const [event, listener] of registrations)
    chrome.webRequest[event].removeListener(listener);
  const redirected = rows.find((row) => row.event === "onBeforeRedirect");
  h.record("observed lifecycle", {
    before: rows.some((row) => row.event === "onBeforeRequest"),
    redirect: redirected?.statusCode === 302,
    redirectedCompletionKeepsId:
      !!redirected &&
      rows.some(
        (row) =>
          row.event === "onCompleted" && row.requestId === redirected.requestId,
      ),
    error: rows.some((row) => row.event === "onErrorOccurred"),
    exactTab: rows.every((row) => row.tabId === tab.id),
    distinctResources: new Set(rows.map((row) => row.requestId)).size,
  });
}
