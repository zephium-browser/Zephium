export async function run(h) {
  await h.call("storage burst", async () => {
    const values=await Promise.all(Array.from({length:400},(_,index)=>chrome.storage.local.get({[String(index)]:index})));
    return {count:values.length,ordered:values.every((value,index)=>value[index]===index)};
  });
  await h.call("regex burst", async () => {
    const values=await Promise.all(Array.from({length:400},()=>chrome.declarativeNetRequest.isRegexSupported({regex:"test"})));
    return {count:values.length,supported:values.every(value=>value.isSupported===true)};
  });
}
