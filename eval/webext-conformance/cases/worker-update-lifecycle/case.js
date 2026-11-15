export async function run(h) {
  await h.sleep(100);
  const before = await chrome.runtime.sendMessage('status');
  h.record('initial registration',{phases:before.phases,state:before.state,root:before.root});
  await h.call('update channel',()=>chrome.runtime.sendMessage('update'));
  const after = await chrome.runtime.sendMessage('status');
  h.record('woken same registration',{newBoot:before.boot!==after.boot,phases:after.phases,state:after.state,root:after.root});
}
