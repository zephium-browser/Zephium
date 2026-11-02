import { expect, test } from "vitest";
import type { DownloadView } from "$shared/ipc/bindings";
import { downloadKind, filenameParts } from "$domain/downloads";
import { TransferRate, formatTimeLeft, transferLine } from "../lib/transfer";

const entry = (received: number, total: number | null): DownloadView => ({
  id: "d",
  revision: "1",
  created_at: "1",
  filename: "a.zip",
  source: "https://a.example",
  state: "receiving",
  received: String(received),
  total: total === null ? null : String(total),
  error: null,
});

test("the rate settles across readings and starts over when a transfer restarts", () => {
  const rate = new TransferRate();
  expect(rate.observe("d", 0, 0)).toBeNull();
  expect(rate.observe("d", 1000, 1000)).toBe(1000);
  expect(rate.observe("d", 4000, 2000)).toBeCloseTo(1000 * 0.7 + 3000 * 0.3);
  expect(rate.observe("d", 4100, 2100)).toBeCloseTo(1600);
  expect(rate.observe("d", 10, 3000)).toBeNull();
  rate.retain([]);
  expect(rate.observe("d", 500, 4000)).toBeNull();
});

test("a resumed transfer does not include the paused interval in its rate", () => {
  const rate = new TransferRate();
  rate.observe("d", 0, 0);
  expect(rate.observe("d", 1000, 1000)).toBe(1000);
  rate.retain([{ id: "d", state: "receiving" }]);
  expect(rate.observe("d", 2000, 2000)).toBe(1000);
  rate.retain([{ id: "d", state: "paused" }]);
  expect(rate.observe("d", 2000, 601_000)).toBeNull();
  expect(rate.observe("d", 3000, 602_000)).toBe(1000);
});

test("the line says how far along and how long is left once it knows", () => {
  expect(transferLine(entry(512, null), 100)).toBe("512 B");
  expect(transferLine(entry(1024, 4096), null)).toBe("1 KB of 4 KB");
  expect(transferLine(entry(0, 120_000), 1000)).toMatch(/^0 B of 117.2 KB · 2 min left$/u);
  expect(formatTimeLeft(0.2)).toMatch(/^1 sec/u);
  expect(formatTimeLeft(7200)).toMatch(/^2 hr/u);
});

test("a file's kind comes from its name", () => {
  expect(downloadKind("Photo.HEIC")).toBe("image");
  expect(downloadKind("setup.dmg")).toBe("app");
  expect(downloadKind("report.pdf")).toBe("document");
  expect(downloadKind("ledger.xlsx")).toBe("sheet");
  expect(downloadKind(".bashrc")).toBe("file");
  expect(downloadKind("archive")).toBe("file");
});

test("a long name keeps its extension in view", () => {
  expect(filenameParts("Terax_0.8.6_aarch64.dmg")).toEqual({
    stem: "Terax_0.8.6_aarch64",
    extension: ".dmg",
  });
  expect(filenameParts(".bashrc")).toEqual({ stem: ".bashrc", extension: "" });
  expect(filenameParts("notes.a-very-long-suffix")).toEqual({
    stem: "notes.a-very-long-suffix",
    extension: "",
  });
});
