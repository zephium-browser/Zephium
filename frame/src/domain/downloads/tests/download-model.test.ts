import { describe, expect, it } from "vitest";
import { downloadProgress, formatDownloadBytes } from "../download-model";
import type { DownloadView } from "$shared/ipc/bindings";

const entry: DownloadView = {
  id: "download",
  revision: "00000001",
  created_at: "1",
  filename: "file",
  source: "https://example.com",
  source_is_context: false,
  state: "receiving",
  received: "512",
  total: null,
  error: null,
  insecure: false,
};
describe("native download progress", () => {
  it("keeps an unknown transfer length indeterminate", () => {
    expect(downloadProgress(entry)).toBeUndefined();
    expect(downloadProgress({ ...entry, total: "0" })).toBeUndefined();
    expect(downloadProgress({ ...entry, total: "1024" })).toBe(0.5);
  });
  it("bounds progress when native byte counts differ from response length", () => {
    expect(downloadProgress({ ...entry, total: "100" })).toBe(1);
    expect(formatDownloadBytes("1024")).toContain("KB");
    expect(formatDownloadBytes("invalid")).toBe("—");
  });
});
