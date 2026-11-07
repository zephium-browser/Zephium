// Body of an async function run on a channel's playlists page: the first
// playlist's watch link, with its first video and list, or null.
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const consent = document.querySelector('form[action*="consent"] button');
if (location.hostname.startsWith('consent.') && consent) return { consent: true };
for (let waited = 0; waited < 15000; waited += 250) {
  const link = [...document.querySelectorAll('a[href*="/watch?v="][href*="list=PL"]')]
    .map((element) => new URL(element.href, location.href))
    .find((url) => url.searchParams.get('v') && url.searchParams.get('list'));
  if (link) return { url: `https://www.youtube.com/watch?v=${link.searchParams.get('v')}&list=${link.searchParams.get('list')}` };
  await sleep(250);
}
return { url: null };
