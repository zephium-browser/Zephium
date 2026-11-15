const plain=document.querySelector('#buttonPlain'),formatted=document.querySelector('#buttonFormatted');
const states=[];
for(const button of [plain,formatted]) {
 if(button){button.dispatchEvent(new MouseEvent('mousedown',{bubbles:true}));await new Promise(resolve=>setTimeout(resolve,50));states.push({button:button.id,bodyClass:document.body.className,rawHidden:document.querySelector('#jsonFormatterRaw')?.hidden,parsedHidden:document.querySelector('#jsonFormatterParsed')?.hidden,text:document.body.innerText.slice(0,2048)});}
}
return JSON.stringify({plain:!!plain,formatted:!!formatted,states});
