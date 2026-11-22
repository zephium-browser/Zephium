const boot = crypto.randomUUID();

chrome.runtime.onMessage.addListener((message, _sender, reply) => {
  if (message !== "popup-restart-probe") return false;
  reply({ status: "ready", boot });
  return false;
});
