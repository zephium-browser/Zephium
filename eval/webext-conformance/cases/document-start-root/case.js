export async function run(h) {
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  for (const world of ["MAIN", "ISOLATED"]) {
    await h.call(world, () => chrome.scripting.executeScript({target:{tabId:tab.id},world,func:()=>({start:globalThis.startRoot,firstInline:document.documentElement.dataset.mainBeforeInline,root:document.documentElement.dataset.mainRoot})}));
  }
}
