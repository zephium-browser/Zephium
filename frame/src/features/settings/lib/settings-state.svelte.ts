import type { SettingsSection } from "./settings-model";
let revision = $state(0);
export const selectionRevision = () => revision;
let current = $state<SettingsSection>("general");
let search = $state("");
let target = $state<string | null>(null);
let asked = $state<"credits" | null>(null);
export const section = () => current;
export const query = () => search;
export const highlighted = () => target;
/** A dialog another surface asked Settings to open as it shows, until it opens. */
export const asking = () => asked;
export function ask(dialog: "credits" | null) {
  asked = dialog;
}
export function setQuery(value: string) {
  search = value;
  target = null;
}
export function select(value: SettingsSection, field: string | null = null) {
  revision++;
  current = value;
  search = "";
  target = field;
}

/** Every page another surface may open Settings at, by its id. */
const PAGES = [
  "general",
  "appearance",
  "tabs",
  "newtab",
  "search",
  "privacy",
  "focus",
  "performance",
  "profiles",
  "work",
  "ai",
  "skills",
  "memory",
  "sites",
  "mcp",
  "features",
  "shortcuts",
  "about",
  "account",
] as const satisfies readonly SettingsSection[];

export function handleNativeSection(id: string) {
  const section = id.slice("settings.section.".length);
  if (
    id.startsWith("settings.section.") &&
    (section === "connections" || section.startsWith("connections."))
  ) {
    const service = section.slice("connections.".length);
    select("mcp", service ? `connection.${decodeURIComponent(service).toLowerCase()}` : null);
    return;
  }
  if (id.startsWith("settings.section.") && (PAGES as readonly string[]).includes(section))
    select(section as SettingsSection);
}
