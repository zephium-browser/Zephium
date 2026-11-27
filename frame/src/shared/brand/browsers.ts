import type { BrandArt } from "$shared/ui/BrandMark";
import arc from "./marks/arc.svg";
import brave from "./marks/brave.svg";
import chrome from "./marks/chrome.svg";
import chromium from "./marks/chromium.png";
import comet from "./marks/comet.png";
import dia from "./marks/dia.png";
import edge from "./marks/edge.svg";
import firefox from "./marks/firefox.svg";
import floorp from "./marks/floorp.svg";
import helium from "./marks/helium.png";
import librewolf from "./marks/librewolf.svg";
import opera from "./marks/opera.svg";
import operaGx from "./marks/opera-gx.svg";
import safari from "./marks/safari.svg";
import vivaldi from "./marks/vivaldi.svg";
import waterfox from "./marks/waterfox.svg";
import zen from "./marks/zen.svg";

const MARKS: Readonly<Record<string, BrandArt>> = {
  arc: { light: arc },
  brave: { light: brave },
  chrome: { light: chrome },
  chromium: { light: chromium },
  comet: { light: comet },
  dia: { light: dia },
  edge: { light: edge },
  firefox: { light: firefox },
  floorp: { light: floorp },
  helium: { light: helium },
  librewolf: { light: librewolf },
  opera: { light: opera },
  "opera-gx": { light: operaGx },
  safari: { light: safari },
  vivaldi: { light: vivaldi },
  waterfox: { light: waterfox },
  zen: { light: zen },
};

/** A browser's own mark, by the id an import source reports. A browser this
 *  build has no artwork for gets none, never another brand's. */
export function browserMark(id: string): BrandArt | null {
  return MARKS[id] ?? null;
}
