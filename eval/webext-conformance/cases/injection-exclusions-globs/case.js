export async function run(h) {
  for (const query of [
    "included=1",
    "included=1&skip=1",
    "excluded=included",
    "plain=1",
  ]) {
    const tab = await h.openTab(`${h.origins.a}/page.html?${query}`);
    await h.call(query, () =>
      chrome.scripting.executeScript({
        target: { tabId: tab.id },
        func: () => ({
          marker: document.documentElement.dataset.globMatched ?? null,
        }),
      }),
    );
  }
}
