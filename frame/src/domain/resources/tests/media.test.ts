import { expect, test } from "vitest";
import { mediaUrl, pictureUrl, recordMediaUrl } from "../media";
import type { WorkRecordMediaRefV1 } from "$shared/ipc/bindings";

const PROFILE = "01M3CTWDG20GFZPACD7905RSRJ";
const DIGEST = "a".repeat(64);

test("a picture is asked for at the width it is shown, and a large view asks for more", () => {
  expect(mediaUrl(PROFILE, DIGEST)).toMatch(/\/01M3CTWDG20GFZPACD7905RSRJ\/a{64}\?w=720$/u);
  expect(mediaUrl(PROFILE, DIGEST, 1600)).toMatch(/\?w=1600$/u);
  expect(mediaUrl(PROFILE, "nope")).toBeNull();
});

test("record pictures retain exact local custody and never turn into a remote image fetch", () => {
  const record: WorkRecordMediaRefV1 = {
    profile: PROFILE,
    work: PROFILE,
    execution: PROFILE,
    attempt: PROFILE,
    step: PROFILE,
    extraction: PROFILE,
    record: 0,
    source: DIGEST,
    digest: DIGEST,
    width: 160,
    height: 100,
    origin: "native_crop",
  };
  const url = recordMediaUrl(record)!;
  expect(url).toContain("/record/");
  expect(url).not.toContain("?w=");
  const encoded = url.split("/record/")[1]!;
  expect(JSON.parse(atob(encoded.replaceAll("-", "+").replaceAll("_", "/")))).toEqual(record);
  expect(pictureUrl({ profile: PROFILE, digest: DIGEST, record }, 1600)).toBe(url);
  expect(recordMediaUrl({ ...record, record: 1 })).not.toBe(url);
  expect(recordMediaUrl({ ...record, digest: "b".repeat(64) })).not.toBe(url);
  expect(recordMediaUrl({ ...record, profile: "01M3CTWDG20GFZPACD7905RSRK" })).not.toBe(url);
  expect(recordMediaUrl({ ...record, width: 321 })).toBeNull();
  expect(recordMediaUrl({ ...record, source: "not-a-native-digest" })).toBeNull();
});
