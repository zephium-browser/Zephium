/**
 * Loads PDF.js and its viewer once. The viewer module reads the core library
 * from `globalThis.pdfjsLib`, so that is set before it is imported. The worker
 * is a bundled same-origin file, as the chrome's CSP requires.
 */
import * as pdfjs from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";
import "pdfjs-dist/web/pdf_viewer.css";

pdfjs.GlobalWorkerOptions.workerSrc = workerUrl;
(globalThis as { pdfjsLib?: typeof pdfjs }).pdfjsLib = pdfjs;

let viewer: Promise<typeof import("pdfjs-dist/web/pdf_viewer.mjs")> | null = null;

export function loadPdf() {
  viewer ??= import("pdfjs-dist/web/pdf_viewer.mjs");
  return viewer.then((web) => ({ pdfjs, web }));
}
