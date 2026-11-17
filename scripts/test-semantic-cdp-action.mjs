import { readFileSync } from "node:fs";
import { strict as assert } from "node:assert";
import { runInNewContext } from "node:vm";
import { createRequire } from "node:module";
const frameRequire = createRequire(new URL("../frame/package.json", import.meta.url));
const eslintRequire = createRequire(frameRequire.resolve("eslint"));
const espreeRequire = createRequire(eslintRequire.resolve("espree"));
const { parse } = espreeRequire("acorn");

const source=readFileSync(new URL("../crates/zephium-agentic/assets/semantic-runtime-v1.js",import.meta.url),"utf8");
const runtimeContract = readFileSync(new URL("../crates/zephium-agentic/src/semantic_runtime.rs", import.meta.url), "utf8");
const sourceLimit = runtimeContract.match(/pub const MAX_SEMANTIC_RUNTIME_SOURCE_BYTES: usize = (\d+) \* 1024;/);
assert.ok(sourceLimit, "native install budget is declared in Rust");
assert.ok(Buffer.byteLength(source) <= Number(sourceLimit[1]) * 1024, "fixed source respects its declared native install budget");
const ast=parse(source,{ecmaVersion:"latest"});
const declarations=ast.body[0].expression.callee.body.body;
const names=new Set(["cdpKey","startCdpInputs","nextCdpAction","guardedCdpInputs","cdpRichSelectionInside","runRichFill","cdpAncestry"]);
const helpers=declarations.filter(node=>node.type==="FunctionDeclaration" && names.has(node.id.name)).map(node=>source.slice(node.start,node.end)).join("\n");
assert.equal(declarations.filter(node=>node.type==="FunctionDeclaration" && names.has(node.id.name)).length,names.size);

function harness() {
  const listeners=new Map();
  const context={listeners,JSON,Promise,apply:Reflect.apply,jsonStringify:JSON.stringify,promiseThen:Promise.prototype.then,busy:true,pendingCdpAction:null,
    actionFault:code=>`E2:${code}`,document:{},eventTargetGetter:function(){return this.target;},eventComposedPath:null,
    read:(getter,receiver)=>Reflect.apply(getter,receiver,[]),composedContains:(target,recipient)=>target===recipient,
    eventPreventDefault:function(){this.prevented=true;},eventStopImmediatePropagation:function(){this.stopped=true;},
    eventTargetAddEventListener:function(name,callback){listeners.set(name,callback);},
    eventTargetRemoveEventListener:function(name){listeners.delete(name);},
  };
  runInNewContext(helpers,context);
  return context;
}

{
  const h=harness(); let finished=0;let cleaned=0;
  const steps=[{k:"text",p:{text:"exact private text"}},{k:"key",p:{type:"keyUp"}}];
  const first=h.startCdpInputs({a:17},steps,()=>true,()=>{finished++;return "terminal";},()=>cleaned++);
  assert.equal(JSON.parse(first.slice(3)).i,0);
  assert.equal(h.nextCdpAction(17,0,false),"E2:invalid_request");
  assert.equal(h.nextCdpAction(18,1,false),"E2:invalid_request");
  assert.equal(JSON.parse(h.nextCdpAction(17,1,false).slice(3)).i,1);
  assert.equal(h.nextCdpAction(17,2,false),"terminal");
  assert.equal(h.nextCdpAction(17,2,false),"E2:invalid_request");
  assert.equal(finished,1);assert.equal(cleaned,1);assert.equal(h.busy,false);
}

for (const abort of [false,true]) {
  const h=harness();let unchanged=true;let finished=0;let cleaned=0;
  h.startCdpInputs({a:17},[{k:"text",p:{text:"first"}},{k:"text",p:{text:"second"}}],()=>unchanged,()=>finished++,()=>cleaned++);
  unchanged=abort;
  assert.equal(h.nextCdpAction(17,1,abort),"E2:applied_unverified_beforeinput_revalidation");
  assert.equal(finished,0);assert.equal(cleaned,1);assert.equal(h.pendingCdpAction,null);assert.equal(h.busy,false);
}

{
  const h=harness();const target={};const replacement={};let finished=0;
  h.guardedCdpInputs({a:17,k:"click"},target,[{k:"mouse",p:{}},{k:"mouse",p:{}}],()=>true,()=>finished++);
  const event={target:replacement};h.listeners.get("pointerdown")(event);
  assert.equal(event.prevented,true);assert.equal(event.stopped,true);
  assert.equal(h.nextCdpAction(17,1,false),"E2:applied_unverified_beforeinput_revalidation");
  assert.equal(finished,0);assert.equal(h.listeners.size,0);
}

{
  const h=harness();const target={};let changed=false;let finished=0;
  const validate=started=>started || !changed;
  h.guardedCdpInputs({a:17,k:"press"},target,[{k:"key",p:{}},{k:"key",p:{}}],validate,()=>{finished++;return "terminal";});
  h.listeners.get("keydown")({target});
  changed=true; // An admitted Enter handler may send and clear its draft.
  assert.equal(JSON.parse(h.nextCdpAction(17,1,false).slice(3)).i,1);
  h.listeners.get("keyup")({target});
  assert.equal(h.nextCdpAction(17,2,false),"terminal");
  assert.equal(finished,1);assert.equal(h.listeners.size,0);
}

// Extracted production rich-fill recipe: focus alone is insufficient when an
// asynchronous page changes Selection before the next native input callback.
for (const phase of ["between_steps", "beforeinput_recipient"]) {
  const h=harness();const target={};const decoy={};const selection={};const callbacks=new Map();let finished=0;
  Object.assign(h,{
    cdpPreparing:true,documentExecCommand:()=>{throw Error("native must not execCommand");},
    htmlElementFocus:function(){h.document.active=this;},documentGetSelection:()=>selection,
    documentCreateRange:()=>({}),rangeSelectNodeContents:function(node){this.target=node;},
    selectionRemoveAllRanges:()=>{},selectionAddRange:function(range){this.anchor=range.target;this.focus=range.target;},
    documentActiveGetter:function(){return this.active;},selectionAnchorGetter:function(){return this.anchor;},selectionFocusGetter:function(){return this.focus;},
    nodeChildNodesGetter:()=>[],listLength:list=>list.length,listItem:(list,index)=>list[index],
    stringSplit:String.prototype.split,rangeSetStart:null,rangeSetEnd:null,
    eventTargetAddEventListener:function(name,callback){callbacks.set(this===target ? "target" : "global",callback);},
    eventTargetRemoveEventListener:function(){callbacks.delete(this===target ? "target" : "global");},
  });
  assert.ok(h.runRichFill(target,"first\nsecond",()=>true,()=>true,result=>{finished++;return result;},{a:17}).startsWith("P2:"));
  assert.equal(h.document.active,target);
  if (phase==="between_steps") {
    selection.anchor=decoy;selection.focus=decoy;
  } else {
    // Even a valid selection must not authorize a beforeinput on a decoy.
    const event={target:decoy};callbacks.get("global")(event);
    assert.equal(event.prevented,true);assert.equal(event.stopped,true);
  }
  assert.equal(h.nextCdpAction(17,1,false),"E2:applied_unverified_beforeinput_revalidation");
  assert.equal(finished,0);assert.equal(callbacks.size,0);assert.equal(h.pendingCdpAction,null);
}

{
  const h=harness();const host={parent:h.document};const root={host,type:11,parent:null};const target={parent:root,root,connected:true};host.shadow=root;
  Object.assign(h,{MAX_TREE_DEPTH:32,nodeType:node=>node.type||1,
    nodeParentGetter:function(){return this.parent??null;},nodeConnectedGetter:function(){return this.connected;},
    nodeGetRoot:function(){return this.root;},shadowHostGetter:function(){return this.host;},elementShadowGetter:function(){return this.shadow;}});
  const guard=h.cdpAncestry(target);assert.equal(typeof guard,"function");assert.equal(guard(),true);
  host.shadow={};assert.equal(guard(),false);assert.equal(h.cdpAncestry(target),null);
  host.shadow=root;host.parent={};assert.equal(guard(),false);
}


{
  const h=harness();
  const enter=["Enter","Enter",13];
  assert.deepEqual(JSON.parse(JSON.stringify(h.cdpKey(enter,false).p)),{
    type:"keyDown",key:"Enter",code:"Enter",windowsVirtualKeyCode:13,modifiers:0,text:"\r",unmodifiedText:"\r"
  });
  assert.equal("text" in h.cdpKey(enter,true).p,false);
  assert.equal("unmodifiedText" in h.cdpKey(["Escape","Escape",27],false).p,false);
  const target={};const replacement={};let finished=0;
  h.guardedCdpInputs({a:17,k:"press"},target,[h.cdpKey(enter,false),h.cdpKey(enter,true)],()=>true,()=>finished++);
  h.listeners.get("keydown")({target});
  const event={target:replacement};h.listeners.get("keypress")(event);
  assert.equal(event.prevented,true);assert.equal(event.stopped,true);
  assert.equal(h.nextCdpAction(17,1,false),"E2:applied_unverified_beforeinput_revalidation");
  assert.equal(finished,0);assert.equal(h.listeners.size,0);
}
console.log("Semantic CDP action contracts passed: exact terminal, no replay, revocation, recipient replacement, selection drift, open-root binding and cleanup.");

// Exercise the production discard monitor's tracking hooks with native-shaped
// audio/capture objects. Terminal objects remain strongly reachable by the
// fixture, so this proves pruning rather than depending on garbage collection.
{
  const rust = readFileSync(new URL("../crates/zephium-engine/src/host/scripts.rs", import.meta.url), "utf8");
  const bootstrap = rust.match(/const DISCARD_SAFETY_BOOTSTRAP_JS: &str = r#"([\s\S]*?)"#;/)[1];
  const body = parse(bootstrap, {ecmaVersion:"latest"}).body[0].expression.callee.body.body;
  const wanted = new Set(["pruneTracked", "rememberTracked", "endedTrack", "closedContext", "observeStream", "wrapAudioConstructor", "liveTrack"]);
  const hooks = body.filter(node => node.type === "FunctionDeclaration" && wanted.has(node.id.name)).map(node => bootstrap.slice(node.start,node.end)).join("\n");
  class AudioContext { state = "running"; }
  const h = {apply:Reflect.apply, construct:Reflect.construct, WeakReference:WeakRef, weakDeref:WeakRef.prototype.deref,
    arrayPush:Array.prototype.push, audioContexts: hooks.includes("function rememberTracked(") ? [] : new Set(), captureTracks: hooks.includes("function rememberTracked(") ? [] : new Set(), MAX_TRACKED:4096, uncertain:false, audioHooks:[],
    AudioContext, contextState:function(){return this.state;}, trackReadyState:function(){return this.readyState;},
    callGetter:(getter,value)=>Reflect.apply(getter,value,[]), getTracks:function(){return this.tracks;}};
  runInNewContext(hooks, h);
  const size = list => list instanceof Set ? list.size : list.length;
  h.wrapAudioConstructor("AudioContext");
  const retained = [];
  for (let i=0; i<5000; i++) {
    const context = new h.AudioContext();
    const track = {readyState:"live"};
    h.observeStream({tracks:[track,track]});
    assert.equal(size(h.audioContexts),1);
    assert.equal(size(h.captureTracks),1);
    context.state="closed"; track.readyState="ended";
    retained.push(context,track);
  }
  h.pruneTracked(h.audioContexts,h.closedContext);
  h.pruneTracked(h.captureTracks,h.endedTrack);
  assert.equal(h.audioContexts.length,0); assert.equal(h.captureTracks.length,0);
  assert.equal(h.uncertain,false,"completed calls do not exhaust the live-object budget");
  const running = new h.AudioContext(); let visited=0;
  h.pruneTracked(h.audioContexts,h.closedContext,()=>visited++);
  assert.equal(visited,1); assert.equal(h.audioContexts[0].deref(),running);
  for (let i=0;i<4097;i++) { const track={readyState:"live"}; retained.push(track); h.observeStream({tracks:[track]}); }
  assert.equal(h.captureTracks.length,4096); assert.equal(h.uncertain,true,"too many live captures still refuse discard");
  console.log("discard audio/capture weak tracking passed");
}
