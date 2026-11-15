addEventListener('load',()=>chrome.runtime.onMessage.addListener((message,sender,respond)=>{
  if(message?.probe!=="dom-parser")return;
  const doc=new DOMParser().parseFromString('<!doctype html><title>Parsed</title><p>owned</p><script>globalThis.parsedScriptRan=true;<\/script>', 'text/html');
  respond({title:doc.title,text:doc.querySelector('p').textContent,scriptRan:globalThis.parsedScriptRan===true,tabs:typeof chrome.tabs});
}));
