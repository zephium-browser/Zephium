// Google's consent pages for YouTube, ahead of the bench's consent script.
if (location.hostname === 'consent.youtube.com' || location.hostname.startsWith('consent.google')) {
  const button = [...document.querySelectorAll('button, input[type=submit]')].find((b) => /accept all|alle akzeptieren|tout accepter|zaakceptuj/i.test(b.textContent || b.value || ''));
  button?.click();
}
