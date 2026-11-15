export async function run(h) {
  h.record("extension page isolation", {protocol:new URL(chrome.runtime.getURL("")).protocol,secure:isSecureContext,isolated:crossOriginIsolated,sharedArrayBuffer:typeof SharedArrayBuffer,sharedBytes:typeof SharedArrayBuffer==='function'?new SharedArrayBuffer(8).byteLength:null});
  await h.call("explicit Wasm policy", async()=>{const module=await WebAssembly.compile(new Uint8Array([0,97,115,109,1,0,0,0]));return module instanceof WebAssembly.Module;});
}
