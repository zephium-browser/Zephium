/** Which in-app viewer shows a file, by extension. */
export type ViewerKind = "markdown" | "pdf" | "docx" | "sheet" | "image" | "text" | "other";

const IMAGES = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif"]);
const TEXT = new Set(["txt", "csv", "json", "log", "yaml", "yml", "toml", "xml", "ini", "tsv"]);
const SHEETS = new Set(["xlsx", "xlsm"]);

export function viewerFor(ext: string): ViewerKind {
  const lower = ext.toLowerCase();
  if (lower === "md") return "markdown";
  if (lower === "pdf") return "pdf";
  if (lower === "docx") return "docx";
  if (SHEETS.has(lower)) return "sheet";
  if (IMAGES.has(lower)) return "image";
  if (TEXT.has(lower)) return "text";
  return "other";
}

/** The desktop app that edits a viewed-only Office file. */
export function nativeEditor(ext: string): "Word" | "Excel" | "PowerPoint" | null {
  const lower = ext.toLowerCase();
  if (lower === "docx" || lower === "doc") return "Word";
  if (lower === "xlsx" || lower === "xlsm" || lower === "xls" || lower === "csv") return "Excel";
  if (lower === "pptx" || lower === "ppt") return "PowerPoint";
  return null;
}

export function extOf(path: string): string {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const at = name.lastIndexOf(".");
  return at > 0 ? name.slice(at + 1).toLowerCase() : "";
}

/** The short badge Obsidian-style trees show after a non-note file name. */
export function badgeFor(ext: string): string {
  if (!ext || ext === "md") return "";
  return ext.toUpperCase().slice(0, 4);
}

/** A file name without its Markdown extension, as notes are titled. */
export function displayName(name: string): string {
  return name.toLowerCase().endsWith(".md") ? name.slice(0, -3) : name;
}
