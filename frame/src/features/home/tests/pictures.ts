import type { Place } from "../lib/model";

/** Stand-ins for kept pictures and site marks: drawn here, nothing fetched. */
const HUES: Record<string, string> = {
  "stripe.com": "#635bff",
  "raycast.com": "#ff6363",
  "notion.com": "#191919",
  "obsidian.md": "#7c3aed",
  "figma.com": "#0d99ff",
  "vercel.com": "#111111",
  "aljazeera.net": "#f29400",
  "northwindbank.example": "#2b6cb0",
  "observability.example": "#0e9f9a",
};

function hash(text: string) {
  let value = 7;
  for (const char of text) value = (value * 31 + char.charCodeAt(0)) >>> 0;
  return value;
}

function canvas(width: number, height: number) {
  const element = document.createElement("canvas");
  element.width = width;
  element.height = height;
  return [element, element.getContext("2d")!] as const;
}

function bar(
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  fill: string,
) {
  context.fillStyle = fill;
  context.beginPath();
  context.roundRect(x, y, w, h, h / 2);
  context.fill();
}

function results(seed: number) {
  const [element, context] = canvas(320, 200);
  context.fillStyle = "#ffffff";
  context.fillRect(0, 0, 320, 200);
  bar(context, 24, 14, 150, 14, "#eceef2");
  for (let item = 0; item < 4; item++) {
    const y = 44 + item * 38;
    bar(context, 24, y, 70, 5, "#c9ccd3");
    bar(context, 24, y + 9, 130 + ((seed >> item) % 60), 8, "#3b5bdb");
    bar(context, 24, y + 22, 210, 5, "#dadde3");
  }
  return element.toDataURL("image/png");
}

function site(seed: number, hue: string) {
  const [element, context] = canvas(320, 200);
  const dark = seed % 3 === 0;
  context.fillStyle = dark ? "#111216" : "#fbfbfc";
  context.fillRect(0, 0, 320, 200);
  bar(context, 16, 12, 34, 8, hue);
  for (let link = 0; link < 4; link++)
    bar(context, 150 + link * 38, 13, 26, 6, dark ? "#4a4d57" : "#c9ccd3");
  bar(context, 16, 50, 190, 16, dark ? "#f2f2f5" : "#18181c");
  bar(context, 16, 72, 140, 16, dark ? "#f2f2f5" : "#18181c");
  bar(context, 16, 100, 220, 6, dark ? "#62656f" : "#a9adb6");
  bar(context, 16, 112, 180, 6, dark ? "#62656f" : "#a9adb6");
  bar(context, 16, 134, 64, 18, hue);
  context.fillStyle = dark ? "#1d1f26" : "#eef0f4";
  context.beginPath();
  context.roundRect(16, 166, 288, 60, 8);
  context.fill();
  return element.toDataURL("image/png");
}

const kept = new Map<string, string>();
export function picture(key: string): string | null {
  let made = kept.get(key);
  if (!made) {
    const seed = hash(key);
    const hue = Object.values(HUES)[seed % Object.keys(HUES).length]!;
    made = key.startsWith("results:") ? results(seed) : site(seed, hue);
    kept.set(key, made);
  }
  return made;
}

const marks = new Map<string, ImageData>();
export function mark(place: Place): ImageData | null {
  const hue = HUES[place.host];
  if (!hue) return null;
  let made = marks.get(place.host);
  if (!made) {
    const [, context] = canvas(32, 32);
    context.fillStyle = hue;
    context.beginPath();
    context.roundRect(0, 0, 32, 32, 8);
    context.fill();
    context.fillStyle = "#ffffff";
    context.font = "600 19px -apple-system, system-ui, sans-serif";
    context.textAlign = "center";
    context.textBaseline = "middle";
    context.fillText(Array.from(place.title)[0] ?? "", 16, 17);
    made = context.getImageData(0, 0, 32, 32);
    marks.set(place.host, made);
  }
  return made;
}
