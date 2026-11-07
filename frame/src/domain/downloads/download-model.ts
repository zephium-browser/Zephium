import type { DownloadView } from "$shared/ipc/bindings";

export function downloadProgress(entry: DownloadView): number | undefined {
  const received = Number(entry.received);
  const total = entry.total === null ? NaN : Number(entry.total);
  return Number.isFinite(total) && total > 0 && Number.isFinite(received)
    ? Math.max(0, Math.min(1, received / total))
    : undefined;
}

const integers = new Intl.NumberFormat(undefined, { maximumFractionDigits: 0 });
const fractions = new Intl.NumberFormat(undefined, { maximumFractionDigits: 1 });

export function formatDownloadBytes(value: string | null): string {
  if (value === null) return "—";
  const bytes = Number(value);
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const at = Math.min(4, Math.max(0, Math.floor(Math.log2(Math.max(1, bytes)) / 10)));
  return `${(at === 0 ? integers : fractions).format(bytes / 1024 ** at)} ${units[at]}`;
}

export type DownloadKind =
  "image" | "video" | "audio" | "archive" | "document" | "sheet" | "code" | "app" | "file";

const KINDS: Record<string, DownloadKind> = {};
for (const [kind, extensions] of [
  ["image", "png jpg jpeg gif webp avif heic heif svg bmp tif tiff ico raw psd"],
  ["video", "mp4 mov m4v webm mkv avi wmv"],
  ["audio", "mp3 m4a aac wav flac ogg opus aiff"],
  ["archive", "zip rar 7z tar gz tgz bz2 xz zst"],
  ["document", "pdf doc docx rtf txt md pages odt epub key ppt pptx odp"],
  ["sheet", "csv tsv xls xlsx numbers ods"],
  ["code", "js mjs ts json html css xml yaml yml toml rs py rb go java swift sh sql wasm"],
  ["app", "dmg pkg app exe msi msix deb rpm appimage apk ipa"],
] as const)
  for (const extension of extensions.split(" ")) KINDS[extension] = kind;

/** What sort of file a name suggests, for its glyph. */
export function downloadKind(filename: string): DownloadKind {
  const dot = filename.lastIndexOf(".");
  return (dot > 0 && KINDS[filename.slice(dot + 1).toLowerCase()]) || "file";
}

/** A name split so the extension stays readable when the stem is truncated. */
export function filenameParts(filename: string): { stem: string; extension: string } {
  const dot = filename.lastIndexOf(".");
  if (dot <= 0 || filename.length - dot > 12) return { stem: filename, extension: "" };
  return { stem: filename.slice(0, dot), extension: filename.slice(dot) };
}

/** A folder as people write it: Windows' verbatim `\\?\` prefix dropped. */
export function displayDirectory(path: string): string {
  if (path.startsWith("\\\\?\\UNC\\")) return `\\\\${path.slice(8)}`;
  if (/^\\\\\?\\[A-Za-z]:\\/.test(path)) return path.slice(4);
  return path;
}
