export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  for(let i=0;i<2;i++) {
    const result=await chrome.scripting.executeScript({target:{tabId:tab.id},files:['injected.js']});
    h.record('native API from programmatic world',result[0].result);
  }
}
