/**
 * Returns the quiet, native-chrome representation of one authoritative URL.
 *
 * Rust's synchronous presentation barrier performs the same `URL.host`
 * projection before revealing page content. Keep this helper strict: an
 * invalid authoritative value should render empty instead of echoing
 * untrusted text into privileged chrome.
 */
export function restingAddress(url: string | null | undefined): string {
  if (!url) return "";
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}

export function editingAddress(url: string | null | undefined): string {
  return url ?? "";
}

export type AddressSecurity = "none" | "secure" | "insecure" | "untrusted";

/**
 * Transport standing of one authoritative URL. Anything that is not an exact
 * known web scheme reports `none` rather than guessing, so an internal page or
 * a malformed authoritative value never renders a reassuring lock. An
 * encrypted page whose certificate the person continued past is `untrusted`.
 */
export function addressSecurity(
  url: string | null | undefined,
  certificateException = false,
): AddressSecurity {
  if (!url) return "none";
  try {
    switch (new URL(url).protocol) {
      case "https:":
      case "wss:":
        return certificateException ? "untrusted" : "secure";
      case "http:":
      case "ws:":
        return "insecure";
      default:
        return "none";
    }
  } catch {
    return "none";
  }
}

/** What the site panel is told about the page in front, when it has a site. */
export type SitePage = {
  url: string;
  security: AddressSecurity;
  /** What is wrong with the connection, in words; null when nothing is. */
  warning: string | null;
  capture: { camera: string; microphone: string } | null;
  privateWindow: boolean;
  /** The page's own zoom, once it has one, and what pages show at otherwise. */
  zoom: number | null;
  standardZoom: number;
};
