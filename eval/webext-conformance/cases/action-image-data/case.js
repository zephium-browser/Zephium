export async function run(h) {
  await h.call("path scale selectors",()=>chrome.action.setIcon({path:{32:"icon.png",512:"icon.png",1024:"icon.png"}}));
  const canvas = document.createElement("canvas");canvas.width=16;canvas.height=16;
  const ctx=canvas.getContext("2d");ctx.fillStyle="#aabbcc";ctx.fillRect(0,0,16,16);
  const image=ctx.getImageData(0,0,16,16);
  await h.call("canvas ImageData",()=>chrome.action.setIcon({imageData:image}));
  await h.call("ImageData dictionary",()=>chrome.action.setIcon({imageData:{16:image}}));
  await h.call("array-like pixels",()=>chrome.action.setIcon({imageData:{width:1,height:1,data:{length:4,0:511,1:-1,2:1.5,3:255}}}));
  for (const [label,data] of [["empty dictionary",{}],["missing pixels",{width:1,height:1}],["invalid dimensions",{width:0,height:1,data:[]}],["invalid data length",{width:1,height:1,data:[]}]]) {
    let sync=false,returned;
    try { returned=chrome.action.setIcon({imageData:data}); } catch(error) {sync=true;h.record(label,{sync,message:error.message});}
    if(!sync) await h.call(label,()=>returned);
  }
}
