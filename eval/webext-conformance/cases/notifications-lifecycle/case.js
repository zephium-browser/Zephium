export async function run(h) {
  await h.call("permission", () => chrome.notifications.getPermissionLevel());
  const options = {
    type: "basic",
    iconUrl: "icon.png",
    title: "Fixture",
    message: "Notification lifecycle",
  };
  await h.call("create", () => chrome.notifications.create("same", options));
  await h.call("inventory", () => chrome.notifications.getAll());
  await h.call("update", () =>
    chrome.notifications.update("same", { title: "Updated" }),
  );
  await h.call("clear", () => chrome.notifications.clear("same"));
  await h.call("missing clear", () => chrome.notifications.clear("same"));
  await h.call("missing update", () =>
    chrome.notifications.update("missing", { title: "Updated" }),
  );
  await h.call("empty inventory", () => chrome.notifications.getAll());
}
