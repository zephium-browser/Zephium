type Modifier = "Ctrl" | "Alt" | "Shift" | "Meta";

/** The order platforms print modifiers in: ⌃⌥⇧⌘ on macOS, Ctrl+Alt+Shift
 *  elsewhere. */
const ORDER: readonly Modifier[] = ["Ctrl", "Alt", "Shift", "Meta"];

const MAC_GLYPHS: Record<Modifier, string> = { Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Meta: "⌘" };
const OTHER_NAMES: Record<Modifier, string> = {
  Ctrl: "Ctrl",
  Alt: "Alt",
  Shift: "Shift",
  Meta: "Win",
};

/** Key names as Rust's canonical text spells them, keyed by every alias a
 *  stored accelerator or a key event may use. */
const KEY_ALIASES: Record<string, string> = {
  Comma: ",",
  Period: ".",
  Minus: "-",
  Equal: "=",
  Plus: "=",
  BracketLeft: "[",
  BracketRight: "]",
  Semicolon: ";",
  Quote: "'",
  Slash: "/",
  Backslash: "\\",
  Backquote: "`",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Return: "Enter",
  NumpadEnter: "Enter",
  Esc: "Escape",
};

const KEY_LABELS: Record<string, string> = {
  Up: "↑",
  Down: "↓",
  Left: "←",
  Right: "→",
  Enter: "↵",
  Escape: "Esc",
};

type Parsed = { modifiers: Set<Modifier>; key: string };

function modifierOf(token: string, mac: boolean): Modifier | null {
  switch (token) {
    case "CmdOrCtrl":
    case "CommandOrControl":
      return mac ? "Meta" : "Ctrl";
    case "Cmd":
    case "Command":
    case "Super":
    case "Meta":
      return "Meta";
    case "Ctrl":
    case "Control":
      return "Ctrl";
    case "Alt":
    case "Option":
      return "Alt";
    case "Shift":
      return "Shift";
    default:
      return null;
  }
}

function keyOf(token: string): string {
  const code = /^(?:Key([A-Za-z])|Digit(\d))$/u.exec(token);
  if (code) return (code[1] ?? code[2]!).toUpperCase();
  if (token.length === 1) return token.toUpperCase();
  return KEY_ALIASES[token] ?? token;
}

function parse(accelerator: string, mac: boolean): Parsed | null {
  const text = accelerator.trim();
  if (!text) return null;
  // "Ctrl++" names the plus key, which shares its cap with "=".
  const plus = text.endsWith("++");
  const tokens = (plus ? text.slice(0, -2) : text).split("+").filter(Boolean);
  const last = plus ? "=" : tokens.pop();
  if (last === undefined) return null;
  const modifiers = new Set<Modifier>();
  for (const token of tokens) {
    const modifier = modifierOf(token, mac);
    if (!modifier) return null;
    modifiers.add(modifier);
  }
  return { modifiers, key: keyOf(last) };
}

/** An accelerator such as `CmdOrCtrl+Shift+T` or `Alt+KeyK` as the keycaps a
 *  person presses, modifiers in the platform's own order and key codes read
 *  as the key's own label. */
export function acceleratorKeys(accelerator: string, mac: boolean): string[] {
  const parsed = parse(accelerator, mac);
  if (!parsed) return [];
  const names = mac ? MAC_GLYPHS : OTHER_NAMES;
  return [
    ...ORDER.filter((modifier) => parsed.modifiers.has(modifier)).map((m) => names[m]),
    KEY_LABELS[parsed.key] ?? parsed.key,
  ];
}

/** The modifiers a key event holds, as keycaps in the platform's order. */
export function heldModifiers(event: KeyboardEvent, mac: boolean): string[] {
  const held: Record<Modifier, boolean> = {
    Ctrl: event.ctrlKey,
    Alt: event.altKey,
    Shift: event.shiftKey,
    Meta: event.metaKey,
  };
  const names = mac ? MAC_GLYPHS : OTHER_NAMES;
  return ORDER.filter((modifier) => held[modifier]).map((modifier) => names[modifier]);
}

/** Whether two spellings name the same keys, however each was written. */
export function sameAccelerator(a: string, b: string, mac: boolean): boolean {
  const left = parse(a, mac);
  const right = parse(b, mac);
  return (
    left !== null &&
    right !== null &&
    left.key === right.key &&
    left.modifiers.size === right.modifiers.size &&
    [...left.modifiers].every((modifier) => right.modifiers.has(modifier))
  );
}

const MODIFIER_CODES = new Set([
  "MetaLeft",
  "MetaRight",
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
  "CapsLock",
  "Fn",
]);

/** The accelerator a key press spells, by physical key so it does not change
 *  with the keyboard layout, or null while only modifiers are held. */
export function acceleratorFrom(event: KeyboardEvent, mac: boolean): string | null {
  return spell(event, mac, event.code);
}

const LETTER = /^[a-z]$/iu;

/** A command shortcut as the active Windows layout spells it. WebView2 matches
 *  letters by virtual key, so the physical QWERTY position would turn AZERTY's
 *  A into Q and Z into W. Other platforms and non-letter keys keep their codes. */
export function keymapAcceleratorFrom(
  event: KeyboardEvent,
  mac: boolean,
  windows: boolean,
): string | null {
  const key =
    windows && typeof event.key === "string" && LETTER.test(event.key)
      ? event.key.toUpperCase()
      : event.code;
  return spell(event, mac, key);
}

function spell(event: KeyboardEvent, mac: boolean, key: string): string | null {
  if (!event.code || !key || MODIFIER_CODES.has(event.code)) return null;
  const parts: string[] = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push(mac ? "Cmd" : "Super");
  parts.push(key);
  return parts.join("+");
}
