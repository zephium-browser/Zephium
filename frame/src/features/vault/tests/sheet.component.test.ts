import JSZip from "jszip";
import { describe, expect, it } from "vitest";
import { readWorkbook } from "../lib/sheet";

const NS = 'xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"';
const R = 'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"';

describe("workbook", () => {
  it("reads a workbook with shared strings", async () => {
    const zip = new JSZip();
    zip.file(
      "xl/workbook.xml",
      `<workbook ${NS} ${R}><sheets><sheet name="Marks" sheetId="1" r:id="rId1"/></sheets></workbook>`,
    );
    zip.file(
      "xl/_rels/workbook.xml.rels",
      `<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Target="worksheets/sheet1.xml"/></Relationships>`,
    );
    zip.file(
      "xl/sharedStrings.xml",
      `<sst ${NS}><si><t>Part A</t></si><si><r><t>To</t></r><r><t>tal</t></r></si></sst>`,
    );
    zip.file(
      "xl/worksheets/sheet1.xml",
      `<worksheet ${NS}><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="C1"><v>70</v></c></row><row r="3"><c r="A3" t="s"><v>1</v></c><c r="B3" t="b"><v>1</v></c></row></sheetData></worksheet>`,
    );
    const bytes = await zip.generateAsync({ type: "uint8array" });
    const [sheet] = await readWorkbook(bytes);
    expect(sheet?.name).toBe("Marks");
    expect(sheet?.rows).toEqual([["Part A", "", "70"], [], ["Total", "TRUE"]]);
  });
});
