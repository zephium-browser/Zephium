export async function run(h) {
  await h.call("commands promise", async () =>
    (await chrome.commands.getAll()).sort((a, b) =>
      a.name.localeCompare(b.name),
    ),
  );
  await h.callback("commands callback", (callback) =>
    chrome.commands.getAll(callback),
  );
}
