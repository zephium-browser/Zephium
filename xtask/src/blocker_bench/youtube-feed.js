// Body of an async function run on the YouTube home feed or Shorts: scrolls
// through it and counts ad renderers a person would see.
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const consent = document.querySelector('form[action*="consent"] button');
if (location.hostname.startsWith('consent.') && consent) return { consent: true };
const FEED_ADS = [
  'ytd-ad-slot-renderer', 'ytd-in-feed-ad-layout-renderer', 'ytd-display-ad-renderer',
  'ytd-promoted-sparkles-web-renderer', 'ytd-promoted-video-renderer', 'ytd-banner-promo-renderer',
  'ytd-statement-banner-renderer', 'ytd-video-masthead-ad-v3-renderer', 'ytd-primetime-promo-renderer',
  'ad-slot-renderer', 'ytm-promoted-sparkles-web-renderer',
].join(', ');
const ENFORCEMENT = 'ytd-enforcement-message-view-model, #enforcement-message';
const seen = new Set();
let enforcement = false;
for (let pass = 0; pass < 8; pass += 1) {
  for (const element of document.querySelectorAll(FEED_ADS)) {
    const box = element.getBoundingClientRect();
    if (box.width > 0 && box.height > 0 && (!element.checkVisibility || element.checkVisibility())) seen.add(element);
  }
  enforcement ||= !!document.querySelector(ENFORCEMENT);
  scrollBy(0, innerHeight * 0.9);
  await sleep(700);
}
scrollTo(0, 0);
return { url: location.href, feedAds: seen.size, enforcement };
