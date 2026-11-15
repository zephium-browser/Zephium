import { createRequire } from 'node:module';
import { readFile, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { appendFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { tmpdir } from 'node:os';
const require=createRequire(new URL('../../frame/package.json', import.meta.url));
const {chromium}=require('playwright');
if(require('playwright/package.json').version!=='1.63.0')throw Error('Playwright pin mismatch');
const [name, extension, output]=process.argv.slice(2);
await mkdir(output,{recursive:true});
const profile=await mkdtemp(join(tmpdir(),'pm-chromium-'));
const trace=join(output,name+'.jsonl');
await writeFile(trace,'');
const started=Date.now();let seq=0;const counts=new Map();
function log(kind,data={}){const key=kind+':'+(data.data?.args?.[0]?.value||'');const count=(counts.get(key)||0)+1;counts.set(key,count);if(count>5000)return;appendFileSync(trace,JSON.stringify({seq:++seq,ms:Date.now()-started,kind,...data})+'\n');}
const fixture=await readFile(new URL('./fixtures/breadth/index.html',import.meta.url),'utf8');
const video=await readFile(new URL('./fixtures/breadth/video.webm',import.meta.url));
const mediaInput=await readFile(new URL('./fixtures/breadth/media.js',import.meta.url),'utf8');
const jsonInput=await readFile(new URL('./fixtures/breadth/json.js',import.meta.url),'utf8');
const server=createServer((req,res)=>{if(['/analytics.analytics/analytics.min.js','/run3-control.json'].includes(req.url)){log('filter-fixture-hit',{path:req.url});res.writeHead(200,{'content-type':'application/json','cache-control':'no-store'});res.end('{"ownedFilteringFixture":true}');return}if(req.url==='/video.webm'){res.writeHead(200,{'content-type':'video/webm','content-length':video.length});res.end(video);return}const isJSON=req.url==='/breadth.json';res.writeHead(200,{'content-type':isJSON?'application/json':'text/html','cache-control':'no-store'});res.end(isJSON?'{"fixture":true,"items":[1,2,3]}':fixture)});
await new Promise(r=>server.listen(0,'127.0.0.1',r));
const fixtureURL=`http://127.0.0.1:${server.address().port}/breadth`;
const instrument=`(()=>{
 if(globalThis.__pmTraceInstalled || !globalThis.chrome?.runtime?.id)return;
 Object.defineProperty(globalThis,'__pmTraceInstalled',{value:true});
 const emit=(kind,data)=>{try{__pmTrace(JSON.stringify({kind,...data}))}catch{}};
 const safe=(x,depth=0)=>{if(depth>4)return '[depth]';if(typeof x==='function')return '[function]';if(x===undefined)return '[undefined]';if(x===null||typeof x!=='object')return typeof x==='string'?x.slice(0,2000):x;if(x instanceof Error)return {name:x.name,message:x.message,stack:x.stack?.slice(0,3000)};if(Array.isArray(x))return x.slice(0,40).map(v=>safe(v,depth+1));const o={};for(const k of Object.keys(x).slice(0,40)){try{o[k]=safe(x[k],depth+1)}catch{}}return o};
 let n=0;const seen=new WeakSet();
 function walk(obj,path,depth){if(!obj||seen.has(obj)||depth>3)return;seen.add(obj);const keys=new Set();for(let proto=obj;proto&&proto!==Object.prototype;proto=Object.getPrototypeOf(proto))for(const key of Object.getOwnPropertyNames(proto))if(key!=='constructor')keys.add(key);for(const key of keys){let val;try{val=obj[key]}catch{continue}const api=path+'.'+key;if(typeof val==='function'){
 try{obj[key]=function(...args){const call=++n;emit('api-call',{call,api,args:safe(args)});
 const passed=args.map(a=>typeof a==='function'&&!['addListener','removeListener','hasListener'].includes(key)?function(...reply){emit(key==='addListener'?'listener-delivery':'api-callback',{call,api,args:safe(reply),lastError:chrome.runtime.lastError?.message});return Reflect.apply(a,this,reply)}:a);
 try{const result=Reflect.apply(val,obj,passed);if(result&&typeof result.then==='function')return result.then(v=>{emit('api-result',{call,api,value:safe(v)});return v},e=>{emit('api-reject',{call,api,error:safe(e)});throw e});emit('api-result',{call,api,value:safe(result)});return result}catch(e){emit('api-throw',{call,api,error:safe(e)});throw e}}}catch(e){emit('instrument-refused',{api,error:String(e)})}
 }else if(val&&typeof val==='object')walk(val,api,depth+1)}}
 walk(chrome,'chrome',0);emit('instrumented',{url:globalThis.location?.href,extensionId:chrome.runtime.id});
 addEventListener('unhandledrejection',e=>emit('unhandled-rejection',{reason:safe(e.reason)}));
 addEventListener('error',e=>emit('uncaught',{message:e.message,filename:e.filename,line:e.lineno,error:safe(e.error)}));
})();`;
let context,ws;const pending=new Map(),targets=new Map(),scripts=new Map(),contexts=new Map();let command=0;
try{
 context=await chromium.launchPersistentContext(profile,{channel:'chromium',headless:false,chromiumSandbox:true,ignoreDefaultArgs:['--disable-extensions'],locale:'en-US',viewport:{width:1000,height:700},args:['--ozone-platform=x11','--remote-debugging-port=0','--enable-unsafe-extension-debugging','--disable-background-networking','--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE localhost, EXCLUDE 127.0.0.1','--proxy-server=http://127.0.0.1:9','--proxy-bypass-list=localhost;127.0.0.1','--password-store=basic'],timeout:15000});
 log('engine',{version:context.browser().version(),playwright:'1.63.0',revision:'1243',headless:false,extension,fixtureURL,chromiumSandbox:true});
 const [port,path]=(await readFile(join(profile,'DevToolsActivePort'),'utf8')).trim().split('\n');
 ws=new WebSocket(`ws://127.0.0.1:${port}${path}`);await new Promise((r,j)=>{ws.onopen=r;ws.onerror=j});
 function send(method,params={},sessionId){return new Promise((resolve,reject)=>{const id=++command;const timer=setTimeout(()=>{pending.delete(id);reject(Error('CDP timeout '+method))},10000);pending.set(id,{resolve,reject,timer});ws.send(JSON.stringify({id,method,params,...(sessionId?{sessionId}:{})}))})}
 async function event(m){const p=m.params,s=m.sessionId,t=targets.get(s);if(m.method==='Target.attachedToTarget'){
 targets.set(p.sessionId,p.targetInfo);log('target',{session:p.sessionId,target:p.targetInfo,waiting:p.waitingForDebugger});
 const cs=p.sessionId;

 for(const method of ['Runtime.enable','Debugger.enable','Network.enable','Log.enable'])await send(method,{},cs).catch(e=>log('cdp-unavailable',{session:cs,method,error:e.message}));
 if(['page','iframe'].includes(p.targetInfo.type))await send('Page.enable',{},cs);
 await send('Runtime.addBinding',{name:'__pmTrace'},cs);
 if(!process.env.PM_RAW)await send('Debugger.setInstrumentationBreakpoint',{instrumentation:'beforeScriptExecution'},cs);
 await send('Runtime.runIfWaitingForDebugger',{},cs);return;
 }
 if(m.method==='Runtime.executionContextCreated')contexts.set(s+':'+p.context.id,p.context);
 if(m.method==='Runtime.bindingCalled'&&p.name==='__pmTrace'){log('instrument',{session:s,target:t,context:p.executionContextId,event:JSON.parse(p.payload)});return}
 if(m.method==='Debugger.scriptParsed'){scripts.set(s+':'+p.scriptId,p);if(p.url.startsWith('chrome-extension:'))log('script-parsed',{session:s,url:p.url,scriptId:p.scriptId,context:p.executionContextId,hash:p.hash});return}
 if(m.method==='Debugger.paused'){
 log('paused',{session:s,reason:p.reason,data:p.data,frames:p.callFrames.map(f=>({url:f.url,location:f.location}))});
 const f=p.callFrames[0],script=scripts.get(s+':'+f?.location.scriptId);
 if(script?.url.startsWith('chrome-extension:'))log('script-evaluation',{session:s,target:t,url:script.url,scriptId:script.scriptId,context:script.executionContextId,reason:p.reason});
 if(!process.env.PM_RAW && f && script?.url.startsWith('chrome-extension:') && t?.type==='service_worker')await send('Debugger.evaluateOnCallFrame',{callFrameId:f.callFrameId,expression:instrument,returnByValue:true,silent:true},s).then(r=>{if(r.exceptionDetails)log('instrument-exception',{session:s,details:r.exceptionDetails})}).catch(e=>log('instrument-error',{session:s,error:e.message}));
 await send('Debugger.resume',{},s).catch(()=>{});return;
 }
 if(['Runtime.executionContextCreated','Runtime.exceptionThrown','Runtime.consoleAPICalled','Log.entryAdded','Network.requestWillBeSent','Network.responseReceived','Network.loadingFailed','Page.frameNavigated'].includes(m.method)){
 let data=p;if(m.method==='Network.requestWillBeSent')data={requestId:p.requestId,frameId:p.frameId,loaderId:p.loaderId,url:p.request.url,method:p.request.method,documentURL:p.documentURL,type:p.type,initiator:p.initiator};
 if(m.method==='Network.responseReceived')data={requestId:p.requestId,url:p.response.url,status:p.response.status,mimeType:p.response.mimeType,type:p.type};
 log(m.method,{session:s,target:t,data});
 }
 }
 ws.onmessage=({data})=>{const m=JSON.parse(data);if(m.id){const p=pending.get(m.id);if(!p)return;pending.delete(m.id);clearTimeout(p.timer);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result)}else event(m).catch(e=>log('capture-error',{method:m.method,error:e.message}))};
 await send('Target.setAutoAttach',{autoAttach:true,waitForDebuggerOnStart:true,flatten:true,filter:[{type:'browser',exclude:true},{type:'browser_ui',exclude:true},{type:'tab',exclude:true},{}]});
 if(name==='ddkjiahejlhfcafbddmgiahcphecmpfh'){const absent=await context.newPage();await absent.goto(fixtureURL);const filter=await readFile(new URL('./fixtures/breadth/filter.js',import.meta.url),'utf8');log('interaction',{phase:'filtering-absent',state:await absent.evaluate('(async()=>{'+filter+'})()')});await absent.close();}
 const {id}=await send('Extensions.loadUnpacked',{path:extension});log('installed',{id});if(id!==name)throw Error('Loaded identity differs from signed CRX publisher identity');
 const page=await context.newPage();await page.goto(fixtureURL);log('fixture-open',{url:fixtureURL});for(const initial of context.pages()){if(initial!==page&&initial.url()==='about:blank')await initial.close();}
 await new Promise(r=>setTimeout(r,5000));
 for(const installedPage of context.pages()){if(installedPage.url().startsWith(`chrome-extension://${id}/`))await installedPage.waitForLoadState('load');}
 if(name==='nkbihfbeogaeaoehlefnkodbefgpgknn'){const provider=await readFile(new URL('./fixtures/breadth/provider.js',import.meta.url),'utf8');log('interaction',{phase:'wallet-provider',state:await page.evaluate('(async()=>{'+provider+'})()')});}
 log('interaction',{phase:'media',state:await page.evaluate('(()=>{'+mediaInput+'})()')});
 if(name==='dbepggeogbaibhgnhhndojpepiihcmeb'){
   const prepare=await readFile(new URL('./fixtures/breadth/prepare-hints.js',import.meta.url),'utf8');
   const hints=await readFile(new URL('./fixtures/breadth/hints.js',import.meta.url),'utf8');
   await page.bringToFront();await page.evaluate('(()=>{'+prepare+'})()');await page.keyboard.press('f');await page.waitForTimeout(500);
   log('interaction',{phase:'vimium-hints',state:await page.evaluate('(()=>{'+hints+'})()')});
   await page.keyboard.press('Escape');await page.waitForTimeout(100);log('interaction',{phase:'vimium-hints-closed',state:await page.evaluate('(()=>{'+hints+'})()')});
 }
 log('installation-pages-loaded');
 const {targetInfos}=await send('Target.getTargets',{filter:[{type:'tab',exclude:false}]});
 const tab=targetInfos.find(t=>t.url===fixtureURL);await page.bringToFront();
 log('action-before',{id,targetId:tab?.targetId});
 await send('Extensions.triggerAction',{id,targetId:tab.targetId}).then(()=>log('action-after')).catch(e=>log('action-error',{error:e.message}));
 await new Promise(r=>setTimeout(r,15000));
 async function snapshot(phase){const live=new Map((await send('Target.getTargets')).targetInfos.map(t=>[t.targetId,t]));for(const [s,old]of targets){const t=live.get(old.targetId)||old;if((t.type!=='page'&&!(t.type==='other'&&t.url.startsWith('chrome-extension:')))||!live.has(t.targetId))continue;try{const result=await send('Runtime.evaluate',{expression:`JSON.stringify({url:location.href,ready:document.readyState,path:location.pathname,dark:getComputedStyle(document.body).backgroundColor,cookieBanner:(()=>{const e=document.getElementById('cookieBanner'),s=document.getElementById('fixtureSentinel');return e?{exists:true,display:getComputedStyle(e).display,visibility:getComputedStyle(e).visibility,rects:e.getClientRects().length,sentinelVisible:!!s?.getClientRects().length}:null})(),videoSpeed:document.querySelector('video')?.playbackRate,text:document.body?.innerText.slice(0,2048),controls:[...document.querySelectorAll('button,[role=button],a[href],input,select,textarea')].filter(e=>e.getClientRects().length&&!e.disabled).slice(0,40).map(e=>(e.getAttribute('aria-label')||e.textContent||e.getAttribute('placeholder')||'').trim().slice(0,120))})`,returnByValue:true},s);log('page-state',{phase,session:s,state:result.result.value})}catch(e){log('snapshot-error',{session:s,error:e.message})}}}
 await snapshot('action-20-seconds');
 const manifest=JSON.parse(await readFile(join(extension,'manifest.json'),'utf8'));
 const popupPath=(manifest.action||manifest.browser_action)?.default_popup;
 if(popupPath){const forced=await context.newPage();await page.bringToFront();log('forced-popup-before',{path:popupPath});await forced.goto(`chrome-extension://${id}/${popupPath}`).catch(e=>log('forced-popup-navigation-error',{error:e.message}));await new Promise(r=>setTimeout(r,5000));await snapshot('forced-popup');if(name==='ddkjiahejlhfcafbddmgiahcphecmpfh'){const config=await readFile(new URL('./fixtures/breadth/filter-config.js',import.meta.url),'utf8');log('interaction',{phase:'filtering-configuration',state:await forced.evaluate('(async()=>{'+config+'})()')});}await forced.close()}
 const optionsPath=manifest.options_ui?.page||manifest.options_page;
 if(optionsPath){const options=await context.newPage();await options.goto(`chrome-extension://${id}/${optionsPath}`).catch(e=>log('options-navigation-error',{error:e.message}));await new Promise(r=>setTimeout(r,5000));await snapshot('options');await options.close()}
 if(name==='ddkjiahejlhfcafbddmgiahcphecmpfh'){const filter=await readFile(new URL('./fixtures/breadth/filter.js',import.meta.url),'utf8');log('interaction',{phase:'filtering',state:await page.evaluate('(async()=>{'+filter+'})()')});}
 await page.goto(fixtureURL.replace('/breadth','/breadth.json'));await new Promise(r=>setTimeout(r,3000));await snapshot('json-document');log('interaction',{phase:'json-controls',state:await page.evaluate('(async()=>{'+jsonInput+'})()')});
 if(manifest.chrome_url_overrides?.newtab){const newtab=await context.newPage();await newtab.goto('chrome://newtab/').catch(e=>log('newtab-error',{error:e.message}));await new Promise(r=>setTimeout(r,3000));await snapshot('newtab')}
 log('complete');
}catch(e){log('fatal',{error:e.stack});process.exitCode=1}finally{ws?.close();for(const p of pending.values()){clearTimeout(p.timer);p.reject(Error('closed'))}await context?.close();server.closeAllConnections();await new Promise(r=>server.close(r));await rm(profile,{recursive:true,force:true})}
