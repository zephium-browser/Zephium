// Body of an async function, evaluated in the page's main world by every driver.
// `__SITE__` is replaced with the site's own selectors (a JSON array).
const siteSelectors = __SITE__;
const AD_HOSTS = /(^|\.)(doubleclick\.net|googlesyndication\.com|googleadservices\.com|adservice\.google\.[a-z.]+|amazon-adsystem\.com|adnxs\.com|criteo\.(com|net)|taboola\.com|outbrain\.com|pubmatic\.com|rubiconproject\.com|openx\.net|casalemedia\.com|teads\.tv|smartadserver\.com|yieldmo\.com|33across\.com|sharethrough\.com|media\.net|moatads\.com|adsrvr\.org|indexww\.com)$/;
const TRACKER_HOSTS = /(^|\.)(google-analytics\.com|googletagmanager\.com|facebook\.net|connect\.facebook\.net|scorecardresearch\.com|hotjar\.com|segment\.(io|com)|chartbeat\.(com|net)|quantserve\.com|newrelic\.com|nr-data\.net|clarity\.ms|bat\.bing\.com|tiktok\.com|snapchat\.com|ads-twitter\.com|linkedin\.com|demdex\.net|omtrdc\.net|everesttech\.net|krxd\.net|bluekai\.com|adsafeprotected\.com|doubleverify\.com|permutive\.(com|app)|gemius\.pl|cxense\.com)$/;
const GENERIC_SLOTS = [
  'ins.adsbygoogle', '[id^="google_ads_iframe"]', '[id^="div-gpt-ad"]', '[data-google-query-id]',
  '[id^="taboola-"]', '.trc_related_container', '.OUTBRAIN', '[data-ad-slot]', '[data-adunit]',
  'amp-ad', 'amp-embed[type="taboola"]',
];
const shown = (element) => {
  const box = element.getBoundingClientRect();
  if (box.width < 30 || box.height < 30) return false;
  return element.checkVisibility
    ? element.checkVisibility({ opacityProperty: true, visibilityProperty: true })
    : getComputedStyle(element).visibility !== 'hidden';
};
const hostOf = (url) => { try { return new URL(url, location.href).hostname; } catch { return ''; } };
const site = (host) => {
  const labels = host.split('.');
  const country = labels.length > 2 && /^(co|com|org|net|gov|ac)$/.test(labels.at(-2)) && labels.at(-1).length === 2;
  return labels.slice(country ? -3 : -2).join('.');
};
const own = site(location.hostname);

const navigation = performance.getEntriesByType('navigation')[0];
const paint = Object.fromEntries(performance.getEntriesByType('paint').map((e) => [e.name, Math.round(e.startTime)]));
const lcp = await new Promise((resolve) => {
  try {
    new PerformanceObserver((list) => {
      const entries = list.getEntries();
      resolve(Math.round(entries[entries.length - 1].startTime));
    }).observe({ type: 'largest-contentful-paint', buffered: true });
  } catch { resolve(null); }
  setTimeout(() => resolve(null), 100);
});

const resources = performance.getEntriesByType('resource');
let transfer = 0, thirdParty = 0, adRequests = 0, trackerRequests = 0;
for (const entry of resources) {
  transfer += entry.transferSize || 0;
  const host = hostOf(entry.name);
  if (site(host) !== own) thirdParty += 1;
  if (AD_HOSTS.test(host)) adRequests += 1;
  if (TRACKER_HOSTS.test(host)) trackerRequests += 1;
}

const counted = new Set();
let adArea = 0;
const count = (selectors) => {
  let found = 0;
  for (const selector of selectors) {
    let elements;
    try { elements = document.querySelectorAll(selector); } catch { continue; }
    for (const element of elements) {
      if (counted.has(element) || !shown(element)) continue;
      // An ad slot inside another counted slot is the same ad.
      let parent = element.parentElement, nested = false;
      while (parent) { if (counted.has(parent)) { nested = true; break; } parent = parent.parentElement; }
      if (nested) continue;
      counted.add(element);
      const box = element.getBoundingClientRect();
      adArea += box.width * box.height;
      found += 1;
    }
  }
  return found;
};
const adFrames = count([...document.querySelectorAll('iframe')]
  .filter((frame) => AD_HOSTS.test(hostOf(frame.src)) || /^google_ads_iframe/.test(frame.id))
  .map((frame) => (frame.id ? `iframe#${CSS.escape(frame.id)}` : null))
  .filter(Boolean));
let unnamedAdFrames = 0;
for (const frame of document.querySelectorAll('iframe')) {
  if (!frame.id && AD_HOSTS.test(hostOf(frame.src)) && !counted.has(frame) && shown(frame)) {
    counted.add(frame); unnamedAdFrames += 1;
    const box = frame.getBoundingClientRect(); adArea += box.width * box.height;
  }
}
const adSlots = count(GENERIC_SLOTS);
const siteAds = count(siteSelectors);

return {
  url: location.href,
  title: document.title.slice(0, 120),
  dcl: navigation ? Math.round(navigation.domContentLoadedEventEnd) : null,
  load: navigation ? Math.round(navigation.loadEventEnd) : null,
  fcp: paint['first-contentful-paint'] ?? null,
  lcp,
  requests: resources.length + 1,
  thirdParty,
  adRequests,
  trackerRequests,
  transferKb: Math.round(transfer / 1024),
  frames: document.querySelectorAll('iframe').length,
  adFrames: adFrames + unnamedAdFrames,
  adSlots,
  siteAds,
  adAreaKpx: Math.round(adArea / 1000),
  nodes: document.getElementsByTagName('*').length,
};
