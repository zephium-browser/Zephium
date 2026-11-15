const probe = new BroadcastChannel('worker-update-observation');
let started = false;
probe.onmessage = ({data}) => {
  if (data !== 'start' || started) return;
  started = true;
  probe.postMessage({phase:'before'});
  try {
    const pending = self.registration.update();
    probe.postMessage({phase:'returned', thenable:typeof pending?.then === 'function'});
    pending.then(
      value => probe.postMessage({phase:'resolved', sameRegistration:value === self.registration}),
      error => probe.postMessage({phase:'rejected', name:error.name, message:error.message}),
    );
  } catch (error) {
    probe.postMessage({phase:'threw', name:error.name, message:error.message});
  }
};
chrome.runtime.onMessage.addListener((message, _sender, reply) => {
  if (message === 'ready') reply({active:self.registration.active?.state});
});
