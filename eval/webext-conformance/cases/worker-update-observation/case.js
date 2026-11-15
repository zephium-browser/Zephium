export async function run(h) {
  const probe = new BroadcastChannel('worker-update-observation');
  const phases = [];
  probe.onmessage = ({data}) => phases.push(data);
  await h.call('worker ready', () => chrome.runtime.sendMessage('ready'));
  probe.postMessage('start');
  await h.sleep(1000);
  h.record('self update observed for 1000ms', phases);
  probe.close();
  await h.call('page update', async () => {
    const scope = chrome.runtime.getURL('/');
    const registration = await navigator.serviceWorker.getRegistration(scope);
    if (!registration) throw Error('Native extension registration missing');
    return await Promise.race([
      registration.update().then(
        value => ({settlement:'resolved', sameRegistration:value === registration}),
        error => ({settlement:'rejected', name:error.name, message:error.message}),
      ),
      h.sleep(1000).then(() => ({settlement:'unsettled after 1000ms'})),
    ]);
  });
}
