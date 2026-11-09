import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import test from 'node:test';

const source = readFileSync(new URL('../crates/zephium-engine/src/platform/automatic_pip.js', import.meta.url), 'utf8');
function fixture() {
  const listeners = new Map();
  const doc = {
    defaultView: { innerWidth: 1200, innerHeight: 800 },
    pictureInPictureEnabled: true, pictureInPictureElement: null,
    nodes: [],
    createTreeWalker(root) {
      let index = 0;
      return { nextNode: () => root.nodes[index++] || null };
    },
    querySelectorAll(selector) { return this.nodes.filter(node => selector.split(',').includes(node.localName)); },
    querySelector: () => null,
    addEventListener() {}, removeEventListener() {},
    async exitPictureInPicture() {
      this.pictureInPictureElement = null;
      video.getRootNode().pictureInPictureElement = null;
      listeners.get('leavepictureinpicture')?.();
    },
  };
  const video = {
    ownerDocument: doc, localName: 'video', paused: false, ended: false, readyState: 4,
    currentSrc: 'https://example.com/video.mp4', currentTime: 1, duration: 120, playbackRate: 1,
    getRootNode: () => video.root || doc,
    play() { this.paused = false; }, pause() { this.paused = true; },
    muted: false, volume: 1, videoWidth: 640, videoHeight: 360,
    getBoundingClientRect: () => ({ x: 0, y: 0, left: 0, top: 0, right: 640, bottom: 360, width: 640, height: 360 }),
    async requestPictureInPicture() { doc.pictureInPictureElement = video; },
    addEventListener: (type, listener) => listeners.set(type, listener),
    removeEventListener: type => listeners.delete(type),
  };
  doc.nodes.push(video);
  return { doc, video, run: runInNewContext(source, { document: doc }) };
}
test('switch back closes only automatically opened PiP', async () => {
  const { doc, video, run } = fixture();
  assert.equal(await run('leave', 1), true);
  assert.equal(doc.pictureInPictureElement, video);
  await run('return', 2);
  assert.equal(doc.pictureInPictureElement, null);
  doc.pictureInPictureElement = video;
  assert.equal(await run('leave', 3), false);
  await run('return', 4);
  assert.equal(doc.pictureInPictureElement, video);
});
test('return during native entry cancels the late presentation', async () => {
  const { doc, video, run } = fixture();
  let finish;
  video.requestPictureInPicture = () => new Promise(resolve => { finish = () => { doc.pictureInPictureElement = video; resolve(); }; });
  const entering = run('leave', 1);
  await run('return', 2);
  finish();
  assert.equal(await entering, false);
  assert.equal(doc.pictureInPictureElement, null);
  assert.equal(await run('leave', 1), false);
});
test('manual dismissal is respected until returning to the tab', async () => {
  const { doc, run } = fixture();
  await run('leave', 1);
  await doc.exitPictureInPicture();
  assert.equal(await run('leave', 2), false);
  await run('return', 3);
  assert.equal(await run('leave', 4), true);
});
test('rapid return and leave share a pending native entry', async () => {
  const { doc, video, run } = fixture();
  let finish;
  let calls = 0;
  video.requestPictureInPicture = () => new Promise(resolve => {
    calls++;
    finish = () => { doc.pictureInPictureElement = video; resolve(); };
  });
  const entering = run('leave', 1);
  await run('return', 2);
  await run('leave', 3);
  finish();
  assert.equal(await entering, true);
  assert.equal(calls, 1);
  assert.equal(doc.pictureInPictureElement, video);
  await run('return', 4);
  assert.equal(doc.pictureInPictureElement, null);
});
test('WebKit presentation uses the same ownership and return policy', async () => {
  const { doc, video, run } = fixture();
  doc.pictureInPictureEnabled = false;
  video.webkitPresentationMode = 'inline';
  video.webkitSupportsPresentationMode = mode => mode === 'picture-in-picture';
  video.webkitSetPresentationMode = mode => { video.webkitPresentationMode = mode; };
  assert.equal(await run('leave', 1), true);
  assert.equal(video.webkitPresentationMode, 'picture-in-picture');
  await run('return', 2);
  assert.equal(video.webkitPresentationMode, 'inline');
});
test('paused, muted, opted-out and fullscreen video never auto-opens', async () => {
  for (const key of ['paused', 'ended', 'muted', 'disablePictureInPicture']) {
    const { doc, video, run } = fixture();
    video[key] = true;
    assert.equal(await run('leave', 1), false, key);
    assert.equal(doc.pictureInPictureElement, null);
  }
  const { doc, run } = fixture();
  doc.fullscreenElement = {};
  assert.equal(await run('leave', 1), false);
});
test('native rejection leaves subsequent manual use available', async () => {
  const { doc, video, run } = fixture();
  video.requestPictureInPicture = async () => { throw new Error('policy denied'); };
  assert.equal(await run('leave', 1), false);
  doc.pictureInPictureElement = video;
  await run('return', 2);
  assert.equal(doc.pictureInPictureElement, video);
});

// Run the shared media chooser through the real watch and action scripts too.
const media = readFileSync(new URL('../crates/zephium-engine/src/host/media.rs', import.meta.url), 'utf8');
const rustScript = name => media.match(new RegExp(`const ${name}: &str = r#"([\\s\\S]*?)"#;`))[1];
const choose = rustScript('CHOOSE_JS');
const watch = rustScript('MEDIA_WATCH_JS').replace('__CHOOSE__', choose).replace('__LAST__', 'null');
const action = rustScript('MEDIA_ACTION_JS').replace('__CHOOSE__', choose);
function mediaContext(doc) {
  return { document: doc, navigator: {}, window: { setTimeout, clearTimeout } };
}

test('media watch and actions find frames after the shadow node budget is exhausted', async () => {
  const parent = fixture();
  const child = fixture();
  parent.doc.nodes = [
    ...Array.from({ length: 4096 }, () => ({ localName: 'div' })),
    { localName: 'iframe', contentDocument: child.doc },
  ];
  assert.equal(JSON.parse(await runInNewContext(watch, mediaContext(parent.doc))).m.s, 1);
  assert.equal(await runInNewContext(action, mediaContext(parent.doc))('pause', 0), true);
  assert.equal(child.video.paused, true);
});

test('automatic PiP finds frames after the shadow node budget is exhausted', async () => {
  const parent = fixture();
  const child = fixture();
  parent.doc.nodes = [
    ...Array.from({ length: 4096 }, () => ({ localName: 'div' })),
    { localName: 'iframe', contentDocument: child.doc },
  ];
  assert.equal(await parent.run('leave', 1), true);
  assert.equal(child.doc.pictureInPictureElement, child.video);
});

test('frame discovery stays bounded when frames are inaccessible or numerous', async () => {
  for (const kind of ['inaccessible', 'documents']) {
    const parent = fixture();
    const child = fixture();
    const frames = kind === 'inaccessible'
      ? Array.from({ length: 64 }, () => ({ localName: 'iframe', get contentDocument() { throw new Error('cross origin'); } }))
      : Array.from({ length: 7 }, () => ({ localName: 'iframe', contentDocument: { ...fixture().doc, nodes: [] } }));
    parent.doc.nodes = [...frames, { localName: 'iframe', contentDocument: child.doc }];
    assert.equal(await parent.run('leave', 1), false, kind);
    assert.equal(await runInNewContext(action, mediaContext(parent.doc))('pause', 0), false, kind);
  }
});
function shadow(doc, nodes = []) {
  return { ownerDocument: doc, nodes, querySelectorAll: doc.querySelectorAll,
    addEventListener() {}, removeEventListener() {} };
}
function insideShadow(depth = 1) {
  const f = fixture();
  f.doc.nodes = [];
  let root = f.doc;
  for (let i = 0; i < depth; i++) {
    const next = shadow(f.doc);
    root.nodes.push({ localName: 'div', shadowRoot: next });
    root = next;
  }
  root.nodes.push(f.video);
  f.video.root = root;
  return { ...f, root };
}

test('open shadow media is watched, controlled and returned from automatic PiP', async () => {
  const { doc, video, root, run } = insideShadow(2);
  // Standard PiP retargets the document's element to the shadow host.
  video.requestPictureInPicture = async () => {
    root.pictureInPictureElement = video;
    doc.pictureInPictureElement = doc.nodes[0];
  };
  const context = mediaContext(doc);
  const act = runInNewContext(action, context);
  assert.equal(JSON.parse(await runInNewContext(watch, context)).m.s, 1);
  assert.equal(await act('pause', 0), true);
  assert.equal(video.paused, true);
  assert.equal(await act('play', 0), true);
  assert.equal(await run('leave', 1), true);
  assert.equal(JSON.parse(await runInNewContext(watch, context)).m.i, 1);
  await run('return', 2);
  assert.equal(root.pictureInPictureElement, null);
  assert.equal(await act('pip', 0), true);
  assert.equal(root.pictureInPictureElement, video);
  assert.equal(await act('pip', 0), true);
  assert.equal(root.pictureInPictureElement, null);
});

test('shadow discovery is bounded by depth, nodes and root count in both scripts', async () => {
  for (const kind of ['depth', 'nodes', 'roots']) {
    const f = insideShadow(kind === 'depth' ? 9 : 1);
    if (kind === 'nodes') f.doc.nodes.unshift(...Array.from({ length: 4096 }, () => ({ localName: 'div' })));
    if (kind === 'roots') f.doc.nodes.unshift(...Array.from({ length: 64 }, () => ({ localName: 'div', shadowRoot: shadow(f.doc) })));
    assert.equal(await f.run('leave', 1), false, kind);
    assert.equal(await runInNewContext(action, mediaContext(f.doc))('pause', 0), false, kind);
    assert.equal(JSON.parse(await runInNewContext(watch, mediaContext(f.doc))).m, null, kind);
  }
  const f = insideShadow(8);
  assert.equal(await f.run('leave', 1), true, 'the last admitted depth is usable');
  assert.equal(await runInNewContext(action, mediaContext(f.doc))('pause', 0), true);
});

test('shadow players in same-origin frames work while inaccessible frames stay out of scope', async () => {
  const parent = fixture();
  const child = insideShadow();
  parent.doc.nodes = [
    { localName: 'iframe', get contentDocument() { throw new Error('cross origin'); } },
    { localName: 'iframe', contentDocument: child.doc },
    { localName: 'div', shadowRoot: null }, // A closed root is not exposed.
  ];
  assert.equal(await parent.run('leave', 1), true);
  assert.equal(child.doc.pictureInPictureElement, child.video);
  assert.equal(await runInNewContext(action, mediaContext(parent.doc))('pause', 0), true);
  assert.equal(child.video.paused, true);
  parent.doc.nodes.splice(1, 1);
  await parent.run('return', 2);
  assert.equal(await parent.run('leave', 3), false);
  assert.equal(await runInNewContext(action, mediaContext(parent.doc))('pause', 0), false);
});


test('WebKit shadow video keeps automatic ownership and media controls', async () => {
  const { doc, video, run } = insideShadow();
  doc.pictureInPictureEnabled = false;
  video.webkitPresentationMode = 'inline';
  video.webkitSupportsPresentationMode = mode => mode === 'picture-in-picture';
  video.webkitSetPresentationMode = mode => { video.webkitPresentationMode = mode; };
  assert.equal(await run('leave', 1), true);
  assert.equal(video.webkitPresentationMode, 'picture-in-picture');
  assert.equal(JSON.parse(await runInNewContext(watch, mediaContext(doc))).m.i, 1);
  await run('return', 2);
  assert.equal(video.webkitPresentationMode, 'inline');
  const act = runInNewContext(action, mediaContext(doc));
  assert.equal(await act('pip', 0), true);
  assert.equal(video.webkitPresentationMode, 'picture-in-picture');
  assert.equal(await act('pip', 0), true);
  assert.equal(video.webkitPresentationMode, 'inline');
});
