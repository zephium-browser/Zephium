chrome.runtime.onMessage.addListener((message,sender,reply)=>{
 if(message!=="relative navigation")return;
 (async()=>{
  const created=await chrome.tabs.create({url:"welcome.html",active:false});
  const settled = async (id, expected) => { for(let i=0;i<100;i++){const tab=await chrome.tabs.get(id);if(tab.status==='complete'&&tab.url===expected)return tab.url;await new Promise(resolve=>setTimeout(resolve,20));}throw Error('tab did not commit'); };
  const createdCommitted=await settled(created.id, chrome.runtime.getURL("welcome.html"));
  const updated=await chrome.tabs.update(created.id,{url:"welcome.html?updated=1"});
  const updatedCommitted=await settled(created.id, chrome.runtime.getURL("welcome.html?updated=1"));
  const result={created:created.pendingUrl||created.url,updated:updated.pendingUrl||updated.url,createdCommitted,updatedCommitted};
  await chrome.tabs.remove(created.id);reply(result);
 })().catch(error=>reply({error:error.message}));return true;
});
