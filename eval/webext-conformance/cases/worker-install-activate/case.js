export async function run(h) {
  const lifecycle = await h.call("worker lifecycle", () =>
    chrome.runtime.sendMessage({ probe: "worker-lifecycle" }),
  );
  h.assert(
    lifecycle?.events.includes("install") &&
      lifecycle.events.includes("activate"),
    "The worker did not report both install and activate events.",
  );
}
