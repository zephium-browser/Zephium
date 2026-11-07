(async (action, sequence) => {
  // This program runs only in a browser-owned isolated world.
  const key = '__zephiumAutomaticVideoPip';
  const state = globalThis[key] ??= { sequence: 0, owned: null, dismissed: false, pending: false, action: 'return' };
  if (sequence <= state.sequence) return false;
  state.sequence = sequence;
  state.action = action;
  const active = video => video && (video.ownerDocument.pictureInPictureElement === video
    || video.getRootNode().pictureInPictureElement === video
    || video.webkitPresentationMode === 'picture-in-picture');
  const exit = async video => {
    if (!active(video)) return;
    if (video.ownerDocument.pictureInPictureElement === video
      || video.getRootNode().pictureInPictureElement === video)
      await video.ownerDocument.exitPictureInPicture();
    else video.webkitSetPresentationMode('inline');
  };
  if (action !== 'leave') {
    const owned = state.owned;
    state.owned = null;
    state.dismissed = false;
    if (owned) await exit(owned).catch(() => {});
    return false;
  }
  if (state.pending || state.dismissed || active(state.owned) || document.pictureInPictureElement
    || document.fullscreenElement || document.webkitFullscreenElement) return false;

  // Select a substantial playing video, not a muted autoplay thumbnail. Respect
  // the site's PiP policy and leave manual native presentation alone.
  const candidates = [];
  const documents = [document], depths = [0];
  let docCount = 1, nodeCount = 0;
  for (let index = 0; index < documents.length; index++) {
    const root = documents[index], doc = root.ownerDocument || root;
    const walker = doc.createTreeWalker(root, 1);
    let node;
    while (nodeCount < 4096 && (node = walker.nextNode())) {
      nodeCount++;
      if (node.shadowRoot && depths[index] < 8 && documents.length < 64) {
        documents.push(node.shadowRoot); depths.push(depths[index] + 1);
      }
    }
    const frames = root.querySelectorAll('iframe,frame');
    for (let frameIndex = 0; frameIndex < frames.length && frameIndex < 64 && docCount < 8; frameIndex++) {
      const frame = frames[frameIndex];
      try {
        if (frame.contentDocument && !documents.includes(frame.contentDocument) && documents.length < 64) {
          documents.push(frame.contentDocument); depths.push(0); docCount++;
        }
      } catch {}
    }
    if (doc.pictureInPictureElement) return false;
    if (doc.fullscreenElement) continue;
    for (const video of Array.from(root.querySelectorAll('video')).slice(0, 32)) {
      if (active(video)) return false;
      const rect = video.getBoundingClientRect();
      if (video.paused || video.ended || video.readyState < 2 || video.muted || video.volume === 0
        || video.disablePictureInPicture || video.videoWidth === 0 || video.videoHeight === 0
        || rect.width < 160 || rect.height < 90 || rect.bottom <= 0 || rect.right <= 0
        || rect.top >= doc.defaultView.innerHeight || rect.left >= doc.defaultView.innerWidth) continue;
      if (!(doc.pictureInPictureEnabled && typeof video.requestPictureInPicture === 'function')
        && !(typeof video.webkitSupportsPresentationMode === 'function'
          && video.webkitSupportsPresentationMode('picture-in-picture'))) continue;
      candidates.push({ video, area: rect.width * rect.height });
    }
  }
  candidates.sort((a, b) => b.area - a.area);
  const video = candidates[0]?.video;
  if (!video || sequence !== state.sequence) return false;
  state.pending = true;
  try {
    if (video.ownerDocument.pictureInPictureEnabled && typeof video.requestPictureInPicture === 'function')
      await video.requestPictureInPicture();
    else video.webkitSetPresentationMode('picture-in-picture');
    // A rapid return/leave shares the pending native entry. Never start another
    // entry whose presentation could be closed by the earlier completion.
    if (state.action !== 'leave') { await exit(video); return false; }
    state.owned = video;
    const left = () => {
      if (active(video)) return;
      if (state.owned === video) { state.owned = null; state.dismissed = true; }
      video.removeEventListener('leavepictureinpicture', left);
      video.removeEventListener('webkitpresentationmodechanged', left);
    };
    video.addEventListener('leavepictureinpicture', left);
    video.addEventListener('webkitpresentationmodechanged', left);
    return true;
  } catch { return false; }
  finally { state.pending = false; }
})
