import { expect, test } from "vitest";
import type { SitePermissionKindView } from "$shared/ipc/bindings";
import { choiceOf, decisionOf, kindsInUse, shownKinds } from "../lib/site-info";

test("a choice saves a decision, and asking or blocking pop-ups forgets one", () => {
  expect(decisionOf("camera", "allow")).toBe("allow");
  expect(decisionOf("camera", "block")).toBe("block");
  expect(decisionOf("camera", "ask")).toBeNull();
  expect(decisionOf("popups", "allow")).toBe("allow");
  expect(decisionOf("popups", "block")).toBeNull();
});

test("no saved decision reads as Ask, except pop-ups, which stay blocked", () => {
  expect(choiceOf("camera", undefined)).toBe("ask");
  expect(choiceOf("camera", "allow")).toBe("allow");
  expect(choiceOf("camera", "block")).toBe("block");
  expect(choiceOf("popups", undefined)).toBe("block");
  expect(choiceOf("popups", "allow")).toBe("allow");
});

test("only a device the page is using counts as in use", () => {
  expect(kindsInUse(null)).toEqual(new Set());
  expect(kindsInUse({ camera: "none", microphone: "none" })).toEqual(new Set());
  expect(kindsInUse({ camera: "active", microphone: "muted" })).toEqual(
    new Set(["camera", "microphone"]),
  );
  expect(kindsInUse({ camera: "none", microphone: "active" })).toEqual(new Set(["microphone"]));
});

test("rows are the platform's kinds that were decided, used or changed, in its order", () => {
  const kinds = ["camera", "microphone", "geolocation", "popups"] as const;
  const some = (...of: SitePermissionKindView[]) => new Set(of);
  expect(shownKinds(kinds, some())).toEqual([]);
  expect(shownKinds(kinds, some("popups"), some("camera"), some("geolocation"))).toEqual([
    "camera",
    "geolocation",
    "popups",
  ]);
  expect(shownKinds(kinds, some("notifications"))).toEqual([]);
});
