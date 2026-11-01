import type { Plugin } from "vite";

/**
 * PDF.js ships its own viewer stylesheet. Its page and popup shadows are
 * soft drop shadows; the interface system allows those only on surfaces that
 * float, through `--shadow-float`. Rewrite them to that token at import so
 * the vendor sheet follows the same contract as our own styles.
 */
export function vendorCss(): Plugin {
  return {
    name: "zephium-vendor-css",
    enforce: "pre",
    transform(code, id) {
      if (!/[\\/]pdfjs-dist[\\/]web[\\/]pdf_viewer\.css(?:\?|$)/u.test(id)) return null;
      return {
        code: code.replace(/box-shadow\s*:[^;}]+/gu, (declaration) =>
          /none|inset/u.test(declaration) ? declaration : "box-shadow: var(--shadow-float)",
        ),
        map: null,
      };
    },
  };
}
