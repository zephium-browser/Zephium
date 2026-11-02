import { describe, expect, it } from "vitest";
import { icsFileName, planIcs } from "../lib/board/plan-ics";
import type { PlanView } from "../lib/board/types";

const now = new Date("2026-10-10T08:05:09Z");

function plan(steps: PlanView["steps"], title?: string): PlanView {
  return { id: "p1", kind: "plan", title, steps, checkable: false };
}

const unfold = (ics: string) => ics.replace(/\r\n /g, "");

describe("planIcs", () => {
  it("returns null when no step has a date", () => {
    expect(planIcs(plan([{ title: "Pack", kind: "task" }]), { now })).toBeNull();
  });

  it("writes one all-day event per dated step", () => {
    const ics = planIcs(
      plan(
        [
          { title: "Fly", date: "2026-11-02", kind: "task", place: "Lisbon", done: true },
          { title: "Undated", kind: "task" },
          { title: "Hike", date: "2026-11-03", kind: "task", detail: "Bring water" },
        ],
        "Trip",
      ),
      { now },
    );
    expect(ics).toBe(
      [
        "BEGIN:VCALENDAR",
        "VERSION:2.0",
        "PRODID:-//Zephium//Work//EN",
        "CALSCALE:GREGORIAN",
        "METHOD:PUBLISH",
        "X-WR-CALNAME:Trip",
        "BEGIN:VEVENT",
        "UID:p1-0@zephium.work",
        "DTSTAMP:20261010T080509Z",
        "DTSTART;VALUE=DATE:20261102",
        "DTEND;VALUE=DATE:20261103",
        "SUMMARY:Fly",
        "LOCATION:Lisbon",
        "END:VEVENT",
        "BEGIN:VEVENT",
        "UID:p1-2@zephium.work",
        "DTSTAMP:20261010T080509Z",
        "DTSTART;VALUE=DATE:20261103",
        "DTEND;VALUE=DATE:20261104",
        "SUMMARY:Hike",
        "DESCRIPTION:Bring water",
        "END:VEVENT",
        "END:VCALENDAR",
        "",
      ].join("\r\n"),
    );
  });

  it("escapes text values", () => {
    const ics = planIcs(
      plan([{ title: "a;b,c\\d", detail: "one\ntwo", date: "2026-01-01", kind: "task" }]),
      { now },
    )!;
    expect(ics).toContain("SUMMARY:a\\;b\\,c\\\\d\r\n");
    expect(ics).toContain("DESCRIPTION:one\\ntwo\r\n");
  });

  it("folds long lines at 75 octets without splitting characters", () => {
    const title = "日本語のとても長い予定の名前 ".repeat(12) + "é🙂".repeat(20);
    const ics = planIcs(plan([{ title, date: "2026-01-01", kind: "task" }]), { now })!;
    const encoder = new TextEncoder();
    for (const line of ics.split("\r\n"))
      expect(encoder.encode(line).length).toBeLessThanOrEqual(75);
    expect(ics).toContain("\r\n ");
    expect(unfold(ics)).toContain(`SUMMARY:${title}`);
  });

  it("rolls DTEND over month, year and leap day", () => {
    const end = (date: string) =>
      /DTEND;VALUE=DATE:(\d+)/.exec(
        planIcs(plan([{ title: "x", date, kind: "task" }]), { now })!,
      )![1];
    expect(end("2026-12-31")).toBe("20270101");
    expect(end("2028-02-28")).toBe("20280229");
    expect(end("2027-02-28")).toBe("20270301");
  });
});

describe("icsFileName", () => {
  it("avoids Windows device names, including names with an extension", () => {
    for (const title of ["CON", "nul", "COM1", "LPT9", "AUX.txt", "COM¹", "CON .txt"])
      expect(icsFileName(plan([], title))).toBe(`plan ${title}.ics`);
    expect(icsFileName(plan([], "CONFERENCE"))).toBe("CONFERENCE.ics");
    expect(icsFileName(plan([], "Trip."))).toBe("Trip.ics");
  });

  it("keeps Unicode filenames inside the native UTF-8 byte limit", () => {
    for (const title of ["予定".repeat(50), "🙂".repeat(80)]) {
      const name = icsFileName(plan([], title));
      expect(new TextEncoder().encode(name).length).toBeLessThanOrEqual(120);
      expect(name).toMatch(/\.ics$/u);
      expect(name).not.toContain("�");
    }
  });

  it("makes a safe name from the title", () => {
    expect(icsFileName(plan([], "Trip: Lisbon/Porto"))).toBe("Trip Lisbon Porto.ics");
    expect(icsFileName(plan([]))).toBe("plan.ics");
  });
});
