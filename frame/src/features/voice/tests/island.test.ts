import { expect, test } from "vitest";
import { voiceView } from "$shared/testing/voice-session";
import { bars, faultView, freshSplit, island, labelParts, outcomeView } from "../lib/island";

test("nothing shows unless voice is available and something is happening", () => {
  expect(island(null)).toBeNull();
  expect(island(voiceView())).toBeNull();
  for (const availability of ["ai_off", "signed_out", "private", "unsupported"] as const)
    expect(
      island(voiceView({ availability, state: { state: "listening", press: "held" } })),
    ).toBeNull();
  expect(island(voiceView({ mic: true }))).toEqual({ kind: "warm" });
});

test("listening is arming until native says the microphone is open", () => {
  expect(island(voiceView({ state: { state: "arming", press: "latched" } }))).toMatchObject({
    kind: "listening",
    press: "latched",
    arming: true,
  });
  expect(
    island(
      voiceView({
        state: { state: "listening", press: "held" },
        mic: true,
        interim: "  hi there ",
      }),
    ),
  ).toEqual({ kind: "listening", press: "held", arming: false, words: "hi there" });
});

test("a step waits on the person only with a confirm to show; a fault outranks the work", () => {
  const confirm = { block: 0, index: 1, category: "purchase" as const, label: 'Click "Pay"' };
  expect(island(voiceView({ state: { state: "working", work: "confirming" }, confirm }))).toEqual({
    kind: "confirming",
    confirm,
  });
  expect(island(voiceView({ state: { state: "working", work: "confirming" } }))?.kind).toBe(
    "working",
  );
  expect(
    island(voiceView({ state: { state: "working", work: "acting" }, fault: "offline" })),
  ).toEqual({ kind: "fault", fault: "offline" });
});

test("the working line follows the phase, and the step stays apart from it", () => {
  const reading = island(
    voiceView({
      state: { state: "working", work: "reading" },
      phase: { phase: "reading", site: "example.com" },
    }),
  );
  expect(reading).toMatchObject({ kind: "working", step: null });
  expect(reading?.kind === "working" && reading.line).toContain("example.com");
  const acting = island(
    voiceView({ state: { state: "working", work: "thinking" }, step: " Click  ", speaking: true }),
  );
  expect(acting).toMatchObject({ kind: "working", step: "Click", speaking: true });
});

test("settled speech, outcomes and faults keep Rust's order", () => {
  expect(
    island(voiceView({ state: { state: "settled", outcome: "acted" }, speaking: true })),
  ).toMatchObject({ kind: "speaking", outcome: "acted" });
  expect(island(voiceView({ state: { state: "settled", outcome: "partial" } }))).toEqual({
    kind: "settled",
    outcome: "partial",
  });
  expect(
    island(voiceView({ state: { state: "settled", outcome: "acted" }, fault: "timeout" })),
  ).toEqual({ kind: "fault", fault: "timeout" });
  expect(island(voiceView({ fault: "busy", speaking: true }))).toEqual({
    kind: "fault",
    fault: "busy",
  });
});

test("only a seen success reads as done", () => {
  expect(outcomeView("acted").tone).toBe("done");
  expect(outcomeView("answered").tone).toBe("done");
  for (const outcome of ["partial", "not_confirmed", "failed", "needs_you"] as const)
    expect(outcomeView(outcome)).toMatchObject({ tone: "warn", detail: expect.any(String) });
  expect(outcomeView("declined").tone).toBe("quiet");
  expect(outcomeView("cancelled").tone).toBe("quiet");
});

test("each fault names its fix, and only plan and credit faults lead to the account", () => {
  expect(faultView("plan_required", true).remedy).toBe("plans");
  expect(faultView("out_of_credit", false).remedy).toBe("credits");
  expect(faultView("microphone_denied", true).fix).not.toBe(
    faultView("microphone_denied", false).fix,
  );
  for (const fault of ["offline", "timeout", "busy", "no_microphone", "daily_limit"] as const)
    expect(faultView(fault, true)).toMatchObject({ remedy: null, fix: expect.any(String) });
});

test("the newest words split at a word boundary", () => {
  expect(freshSplit("one two three four five")).toEqual({
    said: "one two ",
    newest: "three four five",
  });
  expect(freshSplit("two words")).toEqual({ said: "", newest: "two words" });
  expect(freshSplit("")).toEqual({ said: "", newest: "" });
});

test("bars are a pure, bounded function of the level", () => {
  expect(bars(0.5)).toEqual(bars(0.5));
  expect(bars(0.5)).toHaveLength(5);
  expect(bars(0, 3)).toEqual([0.16, 0.16, 0.16]);
  for (const level of [-1, 2, Number.NaN, 0.9])
    for (const height of bars(level)) {
      expect(height).toBeGreaterThanOrEqual(0.16);
      expect(height).toBeLessThanOrEqual(1);
    }
});

test("a label's quoted control stands apart from its verb", () => {
  expect(labelParts('Click "Reserve" now')).toEqual([
    { text: "Click ", named: false },
    { text: "Reserve", named: true },
    { text: " now", named: false },
  ]);
  expect(labelParts("Scroll down")).toEqual([{ text: "Scroll down", named: false }]);
});
