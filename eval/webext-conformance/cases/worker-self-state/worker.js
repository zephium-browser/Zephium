const worker=self.serviceWorker;
const observed={top:worker.state,scriptURL:worker.scriptURL===chrome.runtime.getURL("worker.js"),stable:self.serviceWorker===worker};
self.addEventListener("install",event=>{observed.install=worker.state;event.waitUntil(Promise.resolve())});
self.addEventListener("activate",event=>{observed.activate=worker.state;event.waitUntil(Promise.reject(Error("intentional activation rejection")))});
chrome.runtime.onMessage.addListener((message,sender,reply)=>{
  if(message!=="state")return;
  let attempts=0;
  const finish=()=>{if(worker.state==="activated"||attempts++===100)reply({...observed,current:worker.state});else setTimeout(finish,10)};
  finish();return true;
});
