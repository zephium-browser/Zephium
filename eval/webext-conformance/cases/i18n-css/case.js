export async function run(h) {
  h.record("long message name", chrome.i18n.getMessage("long_message_".repeat(40)));
  const values = () => {
    const style = getComputedStyle(document.documentElement);
    return Object.fromEntries(["marker", "direction", "owner", "dollars", "nested", "malformed", "after-malformed", "missing", "after-missing", "file", "literal"].map(key => [key, style.getPropertyValue(`--${key}`).trim()]));
  };
  const normalize = rows => rows.map(row => {
    const result = row.result;
    result.ownerMatches = result.owner === `"${chrome.runtime.id}"`;
    delete result.owner;
    return result;
  });
  const tab = await h.openTab(`${h.origins.a}/page.html`);
  const read = async () => normalize(await chrome.scripting.executeScript({target:{tabId:tab.id},func:values}));
  await h.call("manifest CSS", read);
  await h.call("file insertion", () => chrome.scripting.insertCSS({target:{tabId:tab.id},files:["programmatic.css"]}));
  await h.call("file CSS", read);
  await h.call("literal insertion", () => chrome.scripting.insertCSS({target:{tabId:tab.id},css:'html { --literal: "__MSG_marker__"; }'}));
  await h.call("literal CSS", read);
  await h.call("file removal", () => chrome.scripting.removeCSS({target:{tabId:tab.id},files:["programmatic.css"]}));
  await h.call("removed CSS", read);
  const sheet=document.createElement("link");sheet.rel="stylesheet";sheet.href="localized.css";
  await new Promise((resolve,reject)=>{sheet.onload=resolve;sheet.onerror=()=>reject(Error("linked CSS failed"));document.head.append(sheet)});
  h.record("extension linked CSS",normalize([{result:values()}]));
}
