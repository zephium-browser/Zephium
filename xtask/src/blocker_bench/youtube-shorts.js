// Body of an async function run on a /shorts page: moves down the reel one
// short at a time and counts the ad renderers a person would see.
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const SHORTS_ADS = 'ytd-ad-slot-renderer, ytd-in-feed-ad-layout-renderer, ytd-display-ad-renderer, ad-slot-renderer';
const ENFORCEMENT = 'ytd-enforcement-message-view-model, #enforcement-message';
const SHORTS = 8;
const shown = (element) => {
  const box = element.getBoundingClientRect();
  return box.width > 0 && box.height > 0 && box.bottom > 0 && box.top < innerHeight
    && (!element.checkVisibility || element.checkVisibility());
};
// A tab that is not visible ignores the reel's buttons.
let hiddenMs = 0;
while (document.visibilityState !== 'visible' && hiddenMs < 20000) { await sleep(500); hiddenMs += 500; }
const seen = new Set();
const visited = new Set([location.pathname]);
let enforcement = false;
for (let short = 0; short < SHORTS; short += 1) {
  await sleep(1500);
  // An ad slot holds its layout renderer, and the pair is one ad.
  for (const element of document.querySelectorAll(SHORTS_ADS)) {
    if (shown(element) && ![...seen].some((outer) => outer.contains(element))) seen.add(element);
  }
  enforcement ||= !!document.querySelector(ENFORCEMENT);
  const down = document.querySelector('#navigation-button-down button');
  if (down) down.click();
  else document.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowDown', code: 'ArrowDown', keyCode: 40, bubbles: true }));
  await sleep(1000);
  visited.add(location.pathname);
}
return { url: location.href, feedAds: seen.size, shorts: visited.size, enforcement, hiddenMs };
