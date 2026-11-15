export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  const target = { tabId: tab.id, allFrames: true };
  const css = 'html { --removable-sheet: yes; }';
  await h.call('insert before inherited child', () => chrome.scripting.insertCSS({target, css}));
  await chrome.scripting.executeScript({target:{tabId:tab.id},func:()=>new Promise(resolve=>{
    const frame=document.createElement('iframe');frame.srcdoc='<!doctype html><p>Owned child</p>';
    frame.onload=()=>resolve(true);document.body.append(frame);
  })});
  await h.call('remove after inherited child', () => chrome.scripting.removeCSS({target, css}));
  h.record('main style removed',(await chrome.scripting.executeScript({target:{tabId:tab.id},func:()=>getComputedStyle(document.documentElement).getPropertyValue('--removable-sheet').trim()}))[0].result);
}
