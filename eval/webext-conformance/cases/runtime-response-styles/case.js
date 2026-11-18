export async function run(h) {
  for (const style of ["synchronous", "deferred", "promise"]) {
    await h.callback(`${style}.callback`, (done) =>
      chrome.runtime.sendMessage(
        { style, value: { nested: [1, "two", null] } },
        done,
      ),
    );
    await h.call(`${style}.promise`, () =>
      chrome.runtime.sendMessage({
        style,
        value: { nested: [1, "two", null] },
      }),
    );
  }
}
