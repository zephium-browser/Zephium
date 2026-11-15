async function fetchFrom(tabId, url) {
  return chrome.scripting.executeScript({
    target: { tabId },
    world: "MAIN",
    func: async (resource) => {
      try {
        const response = await fetch(resource);
        return {
          ok: response.ok,
          status: response.status,
          text: await response.text(),
        };
      } catch (error) {
        return { error: { name: error.name, message: error.message } };
      }
    },
    args: [url],
  });
}

export async function run(h) {
  const permitted = await h.openTab(`${h.origins.a}/page.html?war-permitted`);
  const denied = await h.openTab(`${h.origins.b}/page.html?war-denied`);
  const resource = chrome.runtime.getURL("public.txt");
  h.record("resource URL", resource);
  try {
    await h.call("resource from permitted origin", () =>
      fetchFrom(permitted.id, resource),
    );
    await h.call("resource from denied origin", () =>
      fetchFrom(denied.id, resource),
    );
    await h.call("private resource from permitted origin", () =>
      fetchFrom(permitted.id, chrome.runtime.getURL("private.txt")),
    );
  } finally {
    await chrome.tabs.remove([permitted.id, denied.id]);
  }
}
