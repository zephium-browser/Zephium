import { commands } from "$shared/ipc/bindings";
import { events } from "$shared/ipc/native-events";

import {
  DEFAULT_CONTRAST,
  DEFAULT_THEME,
  contrastOf,
  forkOf,
  preset,
  resolveTheme,
  sameSpec,
  seedVars,
  type ThemeSpec,
} from "./palette";

export type Appearance = "system" | "light" | "dark";

/** The stored customization, as preference values. */
export type Customization = {
  theme: string;
  custom: string;
  /** `ui.theme-contrast`, 0 to 100. */
  level: string;
  /** The accessibility switch, which takes contrast to its full. */
  contrast: boolean;
  font: string;
  fontSize: string;
  codeFont: string;
};

const media = window.matchMedia("(prefers-color-scheme: light)");
let appearance: Appearance = "system";
let lifecycle = 0;
let initialized = false;
let initializing: Promise<void> | null = null;
let unlisten: (() => void) | null = null;

/** Only this module writes appearance axes on the document root. */
export function applyUiPreferences(tint: string, reduceMotion: boolean) {
  document.documentElement.setAttribute("data-tint", tint);
  setReducedMotion(reduceMotion);
}

export function setReducedMotion(reduceMotion: boolean) {
  document.documentElement.setAttribute("data-reduce-motion", String(reduceMotion));
}

let customization: Customization | null = null;
let draft: ThemeSpec | null = null;
let draftLevel: number | null = null;
const graphite = forkOf(preset(DEFAULT_THEME)!);

export function applyCustomization(next: Customization) {
  customization = next;
  render();
}

const fields = {
  "ui.theme": "theme",
  "ui.theme-custom": "custom",
  "ui.theme-contrast": "level",
  "ui.contrast": "contrast",
  "ui.font": "font",
  "ui.font-size": "fontSize",
  "ui.code-font": "codeFont",
} as const satisfies Record<string, keyof Customization>;
export type CustomizationKey = keyof typeof fields;
export const customizationKeys = Object.keys(fields) as CustomizationKey[];

/** For a surface without the preferences store, such as the launcher: one
 *  stored or projected value at a time. */
export function customize(key: CustomizationKey, value: string) {
  const next: Customization = customization ?? {
    theme: "zephium",
    custom: "",
    level: String(DEFAULT_CONTRAST),
    contrast: false,
    font: "system",
    fontSize: "13",
    codeFont: "system",
  };
  const field = fields[key];
  applyCustomization(
    field === "contrast" ? { ...next, contrast: value === "true" } : { ...next, [field]: value },
  );
}

/** A theme being edited, shown everywhere before it is written. */
export function previewTheme(spec: ThemeSpec | null) {
  draft = spec;
  render();
}

/** A contrast being dragged, shown before it is written. */
export function previewContrast(level: number | null) {
  draftLevel = level;
  render();
}

function render() {
  if (customization === null) return;
  const root = document.documentElement;
  const raised = customization.contrast;
  const spec = draft ?? resolveTheme(customization.theme, customization.custom).spec;
  const level = raised ? 100 : (draftLevel ?? contrastOf(customization.level));
  const vars = seedVars(spec, level);
  // Graphite keeps its literal tokens: only a chosen theme derives a ladder.
  if (level === DEFAULT_CONTRAST && sameSpec(spec, graphite)) {
    root.removeAttribute("data-palette");
    for (const name of Object.keys(vars)) root.style.removeProperty(name);
  } else {
    for (const [name, value] of Object.entries(vars)) root.style.setProperty(name, value);
    root.setAttribute("data-palette", "");
  }
  root.setAttribute("data-font", customization.font);
  root.setAttribute("data-font-size", customization.fontSize);
  root.setAttribute("data-code-font", customization.codeFont);
}

export function applyLayout(axes: { density: "compact" | "comfortable"; contrast: boolean }) {
  document.documentElement.setAttribute("data-density", axes.density);
  document.documentElement.setAttribute("data-contrast", String(axes.contrast));
}

function apply() {
  const light = appearance === "light" || (appearance === "system" && media.matches);
  document.documentElement.setAttribute("data-theme", light ? "light" : "dark");
}

async function initialize(generation: number) {
  // Establish system dark/light synchronously before the first native IPC
  // await so the hidden bootstrap document already has deterministic tokens.
  apply();
  let commandAppearance: Appearance | null = null;
  let commandMaterial: string | null = null;
  const listener = events.uiCommand.listen((e) => {
    if (generation !== lifecycle) return;
    const id = e.payload;
    if (id.startsWith("material.")) {
      const material = id.slice("material.".length);
      if (["none", "vibrancy", "liquid_glass", "acrylic", "mica", "mica_alt"].includes(material)) {
        commandMaterial = material;
        document.documentElement.setAttribute("data-material", material);
      }
      return;
    }
    const next = id.slice("theme.".length);
    if (!id.startsWith("theme.") || (next !== "light" && next !== "dark" && next !== "system")) {
      return;
    }
    commandAppearance = next;
    appearance = next;
    apply();
  });

  try {
    const info = await commands.uiInfo();
    if (generation !== lifecycle) {
      (await listener)();
      return;
    }
    document.documentElement.setAttribute("data-material", commandMaterial ?? info.material);

    const stored = await commands.settingGet("appearance");
    if (generation !== lifecycle) {
      (await listener)();
      return;
    }
    if (
      commandAppearance === null &&
      (stored === "light" || stored === "dark" || stored === "system")
    ) {
      appearance = stored;
    }
    apply();

    const stop = await listener;
    if (generation !== lifecycle) {
      stop();
      return;
    }
    media.addEventListener("change", apply);
    unlisten = stop;
    initialized = true;
  } catch (error) {
    const stop = await listener.catch(() => null);
    stop?.();
    throw error;
  }
}

export function init(): Promise<void> {
  if (initializing !== null) return initializing;
  if (initialized) {
    apply();
    return Promise.resolve();
  }

  const generation = ++lifecycle;
  const task = initialize(generation);
  initializing = task;
  void task.then(
    () => {
      if (initializing === task) initializing = null;
    },
    () => {
      if (initializing === task) initializing = null;
    },
  );
  return task;
}

export function dispose() {
  if (!initialized && initializing === null && unlisten === null) return;

  lifecycle += 1;
  initialized = false;
  initializing = null;
  media.removeEventListener("change", apply);
  unlisten?.();
  unlisten = null;
}
