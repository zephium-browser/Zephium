/**
 * A read-only reader for Excel workbooks (.xlsx/.xlsm): enough to show every
 * sheet's values in a grid. Editing happens in desktop Excel; macros are
 * never read or run. Bounded so a huge sheet cannot freeze the chrome.
 */
import JSZip from "jszip";

export type Sheet = { name: string; rows: string[][]; truncated: boolean };

const MAX_ROWS = 5000;
const MAX_COLUMNS = 200;
const MAIN = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const RELS = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

function xml(text: string): Document {
  return new DOMParser().parseFromString(text, "application/xml");
}

/** "A" -> 0, "AB" -> 27. */
export function columnIndex(reference: string): number {
  let index = 0;
  for (const char of reference) {
    const code = char.charCodeAt(0);
    if (code < 65 || code > 90) break;
    index = index * 26 + (code - 64);
  }
  return index - 1;
}

export function columnName(index: number): string {
  let name = "";
  let n = index + 1;
  while (n > 0) {
    const rest = (n - 1) % 26;
    name = String.fromCharCode(65 + rest) + name;
    n = Math.floor((n - 1) / 26);
  }
  return name;
}

function text(node: Element | null): string {
  if (!node) return "";
  // Rich text keeps its runs in <r><t>; plain text has a single <t>.
  const runs = node.getElementsByTagNameNS(MAIN, "t");
  let value = "";
  for (const run of runs) value += run.textContent ?? "";
  return value;
}

function resolveTarget(target: string): string {
  const path = target.startsWith("/") ? target.slice(1) : `xl/${target}`;
  const parts: string[] = [];
  for (const part of path.split("/")) {
    if (part === "..") parts.pop();
    else if (part !== ".") parts.push(part);
  }
  return parts.join("/");
}

function readSheetXml(source: string, shared: readonly string[]): Omit<Sheet, "name"> {
  const document = xml(source);
  const rows: string[][] = [];
  let truncated = false;
  for (const row of document.getElementsByTagNameNS(MAIN, "row")) {
    const number = Number(row.getAttribute("r") ?? rows.length + 1) - 1;
    if (number >= MAX_ROWS) {
      truncated = true;
      break;
    }
    const cells: string[] = [];
    let next = 0;
    for (const cell of row.getElementsByTagNameNS(MAIN, "c")) {
      const reference = cell.getAttribute("r");
      const column = reference ? columnIndex(reference) : next;
      next = column + 1;
      if (column >= MAX_COLUMNS) {
        truncated = true;
        continue;
      }
      const type = cell.getAttribute("t");
      const raw = cell.getElementsByTagNameNS(MAIN, "v")[0]?.textContent ?? "";
      let value: string;
      if (type === "s") value = shared[Number(raw)] ?? "";
      else if (type === "inlineStr")
        value = text(cell.getElementsByTagNameNS(MAIN, "is")[0] ?? null);
      else if (type === "b") value = raw === "1" ? "TRUE" : "FALSE";
      else value = raw;
      cells[column] = value;
    }
    while (rows.length < number) rows.push([]);
    rows[number] = Array.from(cells, (value) => value ?? "");
  }
  return { rows, truncated };
}

export async function readWorkbook(bytes: ArrayBuffer | Uint8Array): Promise<Sheet[]> {
  const zip = await JSZip.loadAsync(bytes);
  const read = (path: string) => zip.file(path)?.async("string") ?? Promise.resolve(null);
  const workbook = await read("xl/workbook.xml");
  if (!workbook) throw new Error("not a workbook");
  const rels = await read("xl/_rels/workbook.xml.rels");
  const targets = new Map<string, string>();
  if (rels) {
    for (const rel of xml(rels).getElementsByTagName("Relationship")) {
      targets.set(rel.getAttribute("Id") ?? "", rel.getAttribute("Target") ?? "");
    }
  }
  const sharedXml = await read("xl/sharedStrings.xml");
  const shared: string[] = [];
  if (sharedXml) {
    for (const item of xml(sharedXml).getElementsByTagNameNS(MAIN, "si")) shared.push(text(item));
  }
  const sheets: Sheet[] = [];
  const entries = xml(workbook).getElementsByTagNameNS(MAIN, "sheet");
  for (const [index, sheet] of [...entries].entries()) {
    const id = sheet.getAttributeNS(RELS, "id") ?? "";
    const target = targets.get(id);
    const path = target ? resolveTarget(target) : `xl/worksheets/sheet${index + 1}.xml`;
    const source = await read(path);
    if (!source) continue;
    sheets.push({
      name: sheet.getAttribute("name") ?? `Sheet ${index + 1}`,
      ...readSheetXml(source, shared),
    });
  }
  return sheets;
}

/** CSV with quoted fields, for .csv files shown in the same grid. */
export function readCsv(source: string): Sheet {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  for (let index = 0; index < source.length; index++) {
    const char = source[index];
    if (quoted) {
      if (char === '"' && source[index + 1] === '"') {
        field += '"';
        index++;
      } else if (char === '"') quoted = false;
      else field += char;
    } else if (char === '"') quoted = true;
    else if (char === ",") {
      row.push(field);
      field = "";
    } else if (char === "\n" || char === "\r") {
      if (char === "\r" && source[index + 1] === "\n") index++;
      row.push(field);
      rows.push(row);
      row = [];
      field = "";
      if (rows.length >= MAX_ROWS) return { name: "CSV", rows, truncated: true };
    } else field += char;
  }
  if (field || row.length) {
    row.push(field);
    rows.push(row);
  }
  return { name: "CSV", rows, truncated: false };
}
