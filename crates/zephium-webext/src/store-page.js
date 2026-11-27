// Zephium's own "Add to Zephium" stands where the store's "Add to Chrome"
// does, which WebKit shows disabled and WebView2 leaves live. The store also
// asks other browsers to switch to Chrome, in a banner over listings and a
// dialog under the header; both are hidden. Everything is found by its
// controller rather than translated labels, which covers other languages and
// navigation between listings.
//
// The browser shows the button with `show` and learns of clicks from the
// `zephiumStore` handler on macOS, where this runs in a world of its own, or
// through `poll` on WebView2, where it shares the page's world. For the
// latter, everything used after document start is taken before page scripts
// run, the button lives in a closed shadow root, and only trusted clicks
// count.
// Requalify when the store changes its markup.
(() => {
  if (window !== top || location.protocol !== 'https:' ||
      location.hostname !== 'chromewebstore.google.com' || location.port) return;
  const key = '__zephium_store_v1__';
  if (Object.prototype.hasOwnProperty.call(window, key)) return;

  const call = Reflect.apply;
  const getter = (proto, name) => Object.getOwnPropertyDescriptor(proto, name).get;
  const setter = (proto, name) => Object.getOwnPropertyDescriptor(proto, name).set;
  const create = Document.prototype.createElement;
  const queryAll = Document.prototype.querySelectorAll;
  const rects = Element.prototype.getClientRects;
  const later = setTimeout;
  const head = getter(Document.prototype, 'head');
  const root = getter(Document.prototype, 'documentElement');
  const attach = Element.prototype.attachShadow;
  const setAttribute = Element.prototype.setAttribute;
  const append = Node.prototype.appendChild;
  const parent = getter(Node.prototype, 'parentNode');
  const setText = setter(Node.prototype, 'textContent');
  const setDisabled = setter(HTMLButtonElement.prototype, 'disabled');
  const setHidden = setter(HTMLElement.prototype, 'hidden');
  const listen = EventTarget.prototype.addEventListener;
  const exec = RegExp.prototype.exec;
  const freeze = Object.freeze;
  const Observer = MutationObserver;
  const observe = Observer.prototype.observe;
  const handler = globalThis.webkit?.messageHandlers?.zephiumStore;
  const post = handler?.postMessage;

  const LISTING = /^\/detail\/(?:[^/]+\/)?([a-p]{32})$/;
  const ID = /^[a-p]{32}$/;
  const PAGE_STYLE = '[jscontroller="o2G9me"], [jscontroller="h4ilFc"], ' +
    '[jscontroller="ri2s0b"] > :not([data-zephium-store]) { display: none !important; }';
  // The store's Material filled button, through the same theme properties.
  const BUTTON_STYLE = `
    :host { display: inline-block; }
    :host([hidden]) { display: none; }
    button {
      all: initial;
      position: relative;
      box-sizing: border-box;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      min-inline-size: 64px;
      block-size: var(--gm3-button-filled-container-height, 40px);
      margin-block: 4px;
      padding-inline: 24px;
      border-radius: 9999px;
      background-color: var(--gm3-button-filled-container-color, var(--gm3-sys-color-primary, #0b57d0));
      color: var(--gm3-button-filled-label-text-color, var(--gm3-sys-color-on-primary, #fff));
      font-family: var(--gm3-button-filled-label-text-font, "Google Sans Flex", "Google Sans Text", "Google Sans", Roboto, Arial, sans-serif);
      font-size: var(--gm3-button-filled-label-text-size, .875rem);
      font-weight: var(--gm3-button-filled-label-text-weight, 500);
      line-height: 1.25rem;
      white-space: nowrap;
      cursor: pointer;
      user-select: none;
      -webkit-user-select: none;
      transition: box-shadow .2s;
    }
    button::after {
      content: "";
      position: absolute;
      inset: 0;
      border-radius: inherit;
      background: currentColor;
      opacity: 0;
      pointer-events: none;
    }
    button:hover { box-shadow: 0 1px 2px rgba(0, 0, 0, .3), 0 1px 3px 1px rgba(0, 0, 0, .15); }
    button:hover::after { opacity: .08; }
    button:active::after { opacity: .1; }
    button:focus-visible {
      outline: 3px solid var(--gm3-sys-color-secondary, #00639b);
      outline-offset: 2px;
    }
    button:disabled {
      background-color: rgba(var(--gm3-sys-color-on-surface-rgb, 31, 31, 31), .12);
      color: rgba(var(--gm3-sys-color-on-surface-rgb, 31, 31, 31), .38);
      box-shadow: none;
      cursor: default;
    }
    button:disabled::after { opacity: 0; }
  `;

  let state = null;
  let clicked = null;
  let host = null;
  let button = null;
  let styled = false;
  let shown = null;
  let seen = null;

  const listing = () => call(exec, LISTING, [location.pathname])?.[1] ?? null;

  const click = event => {
    if (!event.isTrusted || state === null || state.kind === 'adding') return;
    const id = listing();
    if (id !== state.id) return;
    if (post) call(post, handler, [id]);
    else clicked = id;
  };

  const build = () => {
    host = call(create, document, ['div']);
    call(setAttribute, host, ['data-zephium-store', '']);
    const shadow = call(attach, host, [{mode: 'closed'}]);
    const style = call(create, document, ['style']);
    call(setText, style, [BUTTON_STYLE]);
    button = call(create, document, ['button']);
    call(listen, button, ['click', click]);
    call(append, shadow, [style]);
    call(append, shadow, [button]);
  };

  const render = () => {
    if (host === null) return;
    const visible = state !== null && state.id === listing() ? state : null;
    if (visible === shown) return;
    shown = visible;
    call(setHidden, host, [visible === null]);
    if (visible === null) return;
    call(setText, button, [visible.label]);
    call(setDisabled, button, [visible.kind === 'adding']);
  };

  // The store keeps listings it left in the document, hidden; the button
  // goes to the one on screen.
  const place = () => {
    const anchors = call(queryAll, document, ['[jscontroller="ri2s0b"]']);
    let last = null;
    for (let i = anchors.length - 1; i >= 0; i--) {
      if (call(rects, anchors[i], []).length > 0) return anchors[i];
      last ??= anchors[i];
    }
    return last;
  };

  const update = () => {
    if (!styled) {
      const target = call(head, document, []) ?? call(root, document, []);
      if (target) {
        const style = call(create, document, ['style']);
        call(setText, style, [PAGE_STYLE]);
        call(append, target, [style]);
        styled = true;
      }
    }
    if (state !== null) {
      const anchor = place();
      if (anchor) {
        if (host === null) build();
        if (call(parent, host, []) !== anchor) call(append, anchor, [host]);
      }
    }
    render();
    // Moving between listings may only hide one page and show another.
    const current = listing();
    if (current !== seen) {
      seen = current;
      for (const delay of [100, 500, 1500]) later(update, delay);
    }
  };

  const show = next => {
    if (typeof next !== 'object' || next === null) return;
    const {id, kind, label} = next;
    if (typeof id !== 'string' || call(exec, ID, [id]) === null ||
        (kind !== 'add' && kind !== 'adding' && kind !== 'remove') ||
        typeof label !== 'string' || label.length === 0 || label.length > 200) return;
    state = freeze({id, kind, label});
    update();
  };

  const poll = () => {
    if (clicked !== null) {
      const id = clicked;
      clicked = null;
      return id;
    }
    return state === null ? '?' : '';
  };

  Object.defineProperty(window, key, {value: freeze({show, poll})});
  call(observe, new Observer(update), [document, {childList: true, subtree: true}]);
  update();
  if (post) call(post, handler, ['ready']);
})();
