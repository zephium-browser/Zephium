const provider = globalThis.ethereum;
let accounts;
if (typeof provider?.request === 'function') {
  accounts = await Promise.race([
    provider.request({method: 'eth_accounts'}).then(
      value => ({array: Array.isArray(value), count: Array.isArray(value) ? value.length : null}),
      error => ({error: String(error.message).slice(0, 512)}),
    ),
    new Promise(resolve => setTimeout(() => resolve({timedOut: true}), 2000)),
  ]);
}
return JSON.stringify({present: !!provider, request: typeof provider?.request, isMetaMask: provider?.isMetaMask === true, accounts});
