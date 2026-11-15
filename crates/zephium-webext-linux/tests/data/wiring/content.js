chrome.runtime.sendMessage({ kind: 'hello', href: wiringStarted, top: window === top }, (reply) => {
  const report = { href: location.href, top: window === top, reply, error: chrome.runtime.lastError?.message ?? null };
  fetch('/report?' + encodeURIComponent(JSON.stringify(report)));
});
