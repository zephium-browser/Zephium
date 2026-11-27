// Body of an async function: answers YouTube's consent page or dialog. The
// caller fills in the wait (ms) and whether to accept.
const wait = __WAIT__;
const accept = __ACCEPT__;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const dialog = () => [...document.querySelectorAll('ytd-consent-bump-v2-lightbox .eom-buttons button')];
const needed = accept ? 2 : 1;
for (let waited = 0; waited < wait && !location.hostname.startsWith('consent.') && dialog().length < needed; waited += 250) await sleep(250);
if (location.hostname.startsWith('consent.')) {
  const form = [...document.querySelectorAll('form')].find((f) => f.querySelector(`input[name=set_eom][value=${!accept}]`));
  const button = form?.querySelector('button, input[type=submit]');
  button?.click();
  return { page: 'consent', answered: !!button };
}
const buttons = dialog();
const button = buttons.length >= needed ? buttons[accept ? 1 : 0] : undefined;
button?.click();
return { page: buttons.length ? 'dialog' : 'none', answered: !!button };
