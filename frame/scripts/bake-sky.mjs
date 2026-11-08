// Writes each sky's picture from its source. The app prints it as an ordered
// screen at the screen's own pixels (src/shared/sky/dither.ts), so nothing
// about the look is baked here. Run with `node scripts/bake-sky.mjs`
// after changing a source in assets/sky (needs ffmpeg with libsvtav1).
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

for (const name of ["day", "night"]) {
  const source = fileURLToPath(new URL(`../../assets/sky/${name}-source.png`, import.meta.url));
  const out = fileURLToPath(new URL(`../src/shared/sky/assets/sky-${name}.avif`, import.meta.url));
  execFileSync("ffmpeg", [
    "-v",
    "error",
    "-y",
    "-i",
    source,
    "-pix_fmt",
    "yuv420p",
    "-frames:v",
    "1",
    "-c:v",
    "libsvtav1",
    "-crf",
    "20",
    out,
  ]);
}
