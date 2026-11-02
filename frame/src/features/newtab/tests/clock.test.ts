import { describe, expect, it } from "vitest";
import { clockFace, clockFormat, dayKey, focusSpan, untilNextMinute } from "../lib/clock";

const evening = new Date(2026, 8, 28, 21, 7);
const morning = new Date(2026, 8, 28, 9, 41);

describe("clockFace", () => {
  it("sets the period apart from the figures in a 12-hour clock", () => {
    expect(clockFace(evening, "12h", "en-US")).toEqual({
      time: "9:07",
      period: "PM",
      periodLeading: false,
    });
  });

  it("has no period in a 24-hour clock", () => {
    expect(clockFace(evening, "24h", "en-GB")).toEqual({
      time: "21:07",
      period: null,
      periodLeading: false,
    });
  });

  it("keeps the period on the side the locale puts it", () => {
    const face = clockFace(morning, "12h", "ko-KR");
    expect(face.periodLeading).toBe(true);
    expect(face.period).not.toBeNull();
    expect(face.time).toBe("9:41");
  });

  it("follows the locale when the format is the system's", () => {
    expect(clockFace(evening, "system", "en-GB").period).toBeNull();
    expect(clockFace(evening, "system", "en-US").period).toBe("PM");
  });
});

describe("clockFace with the system's own setting", () => {
  it("follows a 24-hour system even where the locale counts in 12", () => {
    expect(clockFace(evening, "system", "en-US", true)).toEqual({
      time: "21:07",
      period: null,
      periodLeading: false,
    });
  });

  it("follows a 12-hour system even where the locale counts in 24", () => {
    expect(clockFace(evening, "system", "en-GB", false).period?.toLowerCase()).toBe("pm");
  });

  it("leaves the locale in charge when the system cannot say", () => {
    expect(clockFace(evening, "system", "en-US", null).period).toBe("PM");
  });

  it("is overruled by an explicit choice", () => {
    expect(clockFace(evening, "12h", "en-US", true).period).toBe("PM");
    expect(clockFace(evening, "24h", "en-US", false).period).toBeNull();
  });

  it("starts the 24-hour day at 00, not 24", () => {
    const midnight = new Date(2026, 8, 28, 0, 5);
    expect(clockFace(midnight, "24h", "en-US").time).toBe("00:05");
    expect(clockFace(midnight, "system", "en-US", true).time).toBe("00:05");
  });
});

describe("clockFormat", () => {
  it("reads unknown stored values as the system format", () => {
    expect(clockFormat("24h")).toBe("24h");
    expect(clockFormat("12-hour")).toBe("system");
  });
});

describe("untilNextMinute", () => {
  it("lands on the turn of the minute", () => {
    expect(untilNextMinute(120_000)).toBe(60_000);
    expect(untilNextMinute(119_999)).toBe(1);
  });
});

describe("dayKey", () => {
  it("is the local calendar day", () => {
    expect(dayKey(new Date(2026, 8, 28, 23, 59))).toBe("2026-09-28");
    expect(dayKey(new Date(2026, 0, 5, 0, 0))).toBe("2026-01-05");
  });
});

describe("focusSpan", () => {
  it("reads as hours and minutes, dropping whichever part is empty", () => {
    expect(focusSpan(134)).toBe("2h 14m");
    expect(focusSpan(45)).toBe("45m");
    expect(focusSpan(240)).toBe("4h");
  });
});
