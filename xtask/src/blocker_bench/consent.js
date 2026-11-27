// Body of an async function run in the page's main document. `mode` is 'act'
// (wait up to `waitMs` for a consent wall, then accept everything) or 'check'
// (is a wall still showing). `recipe` is the site's own, or null.
const recipe = __RECIPE__;
const mode = __MODE__;
const waitMs = __WAIT__;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const visible = (element) => {
  const box = element.getBoundingClientRect();
  return box.width > 0 && box.height > 0
    && (!element.checkVisibility || element.checkVisibility({ visibilityProperty: true }));
};
const rootOf = (host) => (host ? document.querySelector(host)?.shadowRoot ?? null : document);
const firstVisible = (root, selector) => {
  if (!root) return null;
  try { return [...root.querySelectorAll(selector)].find(visible) ?? null; } catch { return null; }
};
// Sourcepoint lives in a cross-origin iframe: its wall is seen but cannot be clicked.
const CMPS = [
  { name: 'onetrust', dialog: '#onetrust-banner-sdk, #onetrust-pc-sdk', accept: '#onetrust-accept-btn-handler' },
  { name: 'didomi', dialog: '#didomi-popup, .didomi-popup-container, #didomi-notice', accept: '#didomi-notice-agree-button' },
  { name: 'quantcast', dialog: '.qc-cmp2-container', accept: '.qc-cmp2-summary-buttons button[mode=primary]' },
  { name: 'funding-choices', dialog: '.fc-consent-root, .fc-dialog-container', accept: '.fc-cta-consent' },
  { name: 'cookiebot', dialog: '#CybotCookiebotDialog', accept: '#CybotCookiebotDialogBodyLevelButtonLevelOptinAllowAll, #CybotCookiebotDialogBodyButtonAccept' },
  { name: 'usercentrics', shadow: '#usercentrics-root', dialog: '[data-testid=uc-accept-all-button]', accept: '[data-testid=uc-accept-all-button]' },
  { name: 'sourcepoint', dialog: 'iframe[id^=sp_message_iframe], div[id^=sp_message_container]', accept: null },
];
const wall = () => {
  const known = CMPS.find((cmp) => firstVisible(rootOf(cmp.shadow), cmp.dialog));
  const root = rootOf(recipe?.shadow);
  let button = null, selector = null;
  for (const candidate of recipe?.click ?? []) {
    button = firstVisible(root, candidate);
    if (button) { selector = candidate; break; }
  }
  if (!button && known?.accept) {
    button = firstVisible(rootOf(known.shadow), known.accept);
    selector = known.accept;
  }
  if (!known && !button) return null;
  return { cmp: known?.name ?? 'recipe', button, selector };
};
const tcfData = () => new Promise((resolve) => {
  if (typeof window.__tcfapi !== 'function') return resolve(null);
  const timer = setTimeout(() => resolve(null), 1000);
  try {
    window.__tcfapi('getTCData', 2, (data, ok) => { clearTimeout(timer); resolve(ok ? data : null); });
  } catch { clearTimeout(timer); resolve(null); }
});
const tcfShown = async () => !!recipe?.tcf && (await tcfData())?.eventStatus === 'cmpuishown';
const acceptViaApi = () => {
  const apis = [
    ['OneTrust.AllowAll', window.OneTrust, 'AllowAll', []],
    ['Didomi.setUserAgreeToAll', window.Didomi, 'setUserAgreeToAll', []],
    ['Cookiebot.submitCustomConsent', window.Cookiebot, 'submitCustomConsent', [true, true, true]],
    ['UC_UI.acceptAllConsents', window.UC_UI, 'acceptAllConsents', []],
  ];
  for (const [name, owner, method, args] of apis) {
    if (typeof owner?.[method] !== 'function') continue;
    try { owner[method](...args); return name; } catch {}
  }
  return null;
};

if (mode === 'check') return { present: !!wall() || await tcfShown() };

const end = Date.now() + waitMs;
let found = null, tcf = false;
for (;;) {
  found = wall();
  tcf = !found?.button && await tcfShown();
  if (found?.button || tcf || Date.now() >= end) break;
  await sleep(250);
}
if (!found && !tcf) return { shown: false, cmp: null, clicked: null };
if (found?.button) {
  try { found.button.click(); } catch {}
  return { shown: true, cmp: found.cmp, clicked: found.selector };
}
if (recipe?.tcf) {
  const api = acceptViaApi();
  return { shown: true, cmp: found?.cmp ?? 'tcf', clicked: api ? `api:${api}` : null };
}
return { shown: true, cmp: found.cmp, clicked: null };
