import { beforeEach, expect, test, vi } from "vitest";
import { forgetDrafts, onDraftsForgotten, readDraft, writeDraft } from "../drafts";

function memory(): Storage {
  const held = new Map<string, string>();
  return {
    get length() {
      return held.size;
    },
    key: (index) => [...held.keys()][index] ?? null,
    getItem: (key) => held.get(key) ?? null,
    setItem: (key, value) => void held.set(key, value),
    removeItem: (key) => void held.delete(key),
    clear: () => held.clear(),
  };
}

beforeEach(() => {
  vi.useRealTimers();
  vi.stubGlobal("localStorage", memory());
});

test("words come back as they were written, per scope", () => {
  writeDraft("work-composer", ["profile", "home"], "Plan my trip");
  writeDraft("work-composer", ["profile", "studio"], "Another ask");
  expect(readDraft("work-composer", ["profile", "home"])).toBe("Plan my trip");
  expect(readDraft("work-composer", ["profile", "studio"])).toBe("Another ask");
  expect(readDraft("work-composer", ["other", "home"])).toBeNull();
});

test("an empty draft, or one too long to keep, lets the old one go", () => {
  writeDraft("work-composer", ["profile", "home"], "Plan my trip");
  writeDraft("work-composer", ["profile", "home"], "  ");
  expect(readDraft("work-composer", ["profile", "home"])).toBeNull();
  writeDraft("work-composer", ["profile", "home"], "Plan my trip");
  writeDraft("work-composer", ["profile", "home"], "x".repeat(70_000));
  expect(readDraft("work-composer", ["profile", "home"])).toBeNull();
});

test("a draft left alone for a season is not brought back", () => {
  writeDraft("work-composer", ["profile", "home"], "Old words");
  vi.useFakeTimers({ now: Date.now() + 91 * 24 * 60 * 60 * 1000, toFake: ["Date"] });
  expect(readDraft("work-composer", ["profile", "home"])).toBeNull();
  expect(localStorage.length).toBe(0);
});

test("damaged storage reads as no draft and never throws", () => {
  localStorage.setItem("zephium.draft.work-composer.profile.home", "{not json");
  expect(readDraft("work-composer", ["profile", "home"])).toBeNull();
  localStorage.setItem(
    "zephium.draft.work-composer.profile.home",
    JSON.stringify({ v: 2, at: Date.now(), text: "Future" }),
  );
  expect(readDraft("work-composer", ["profile", "home"])).toBeNull();
  vi.stubGlobal("localStorage", {
    getItem: () => {
      throw new Error("blocked");
    },
    setItem: () => {
      throw new Error("full");
    },
  });
  expect(readDraft("work-composer", ["profile", "home"])).toBeNull();
  expect(() => writeDraft("work-composer", ["profile", "home"], "Words")).not.toThrow();
  vi.unstubAllGlobals();
  expect(() => writeDraft("work-composer", ["profile", "home"], "Words")).not.toThrow();
});

test("deleting a space or a profile lets its drafts go and tells live owners", () => {
  writeDraft("work-composer", ["profile", "home"], "Home words");
  writeDraft("work-composer", ["profile", "studio"], "Studio words");
  writeDraft("work-composer", ["other", "home"], "Other words");
  const told: [string, string | undefined][] = [];
  onDraftsForgotten((profile, space) => told.push([profile, space]));

  forgetDrafts("profile", "studio");
  expect(readDraft("work-composer", ["profile", "studio"])).toBeNull();
  expect(readDraft("work-composer", ["profile", "home"])).toBe("Home words");

  forgetDrafts("profile");
  expect(readDraft("work-composer", ["profile", "home"])).toBeNull();
  expect(readDraft("work-composer", ["other", "home"])).toBe("Other words");
  expect(told).toEqual([
    ["profile", "studio"],
    ["profile", undefined],
  ]);
});
