const boot = crypto.randomUUID();
const phases = [];
for (const phase of ['install','activate']) self.addEventListener(phase,()=>phases.push(phase));
chrome.runtime.onMessage.addListener((message, _sender, reply) => {
  if (message === 'status') { reply({boot,phases,state:self.registration.active?.state,root:self.registration.scope===chrome.runtime.getURL('/')}); return; }
  if (message !== 'update') return;
  self.registration.update();
  setTimeout(()=>reply({alive:true}),250);
  return true;
});
