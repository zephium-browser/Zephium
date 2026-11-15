const video=document.querySelector('video');
const controller=[...document.querySelectorAll('vsc-controller')].map(e=>e.shadowRoot).find(root=>root?.querySelector('[data-action="faster"]'));
const before=video?.playbackRate;
controller?.querySelector('[data-action="faster"]')?.click();
const faster=video?.playbackRate;
controller?.querySelector('[data-action="slower"]')?.click();
return JSON.stringify({mainIdleCallback:typeof requestIdleCallback,controller:!!controller,ready:video?.readyState,before,faster,slower:video?.playbackRate});
