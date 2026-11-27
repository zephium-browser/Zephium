import { describe, expect, test } from "vitest";
import type { WorkModelEntry, WorkModelsV1 } from "$shared/ipc/bindings";
import {
  contextSize,
  dollars,
  effortLevels,
  locked,
  pickerGroups,
  priceTier,
  searchEntries,
  shortName,
  usable,
} from "../catalog";
import { entry, models } from "$shared/testing/work-models";

describe("the picker's list", () => {
  test("a provider listing cannot promote a catalog model into another role", () => {
    const known = entry("google", "gemini-3.5-flash-lite", "Gemini 3.5 Flash-Lite", true, [
      "page",
      "light",
    ]);
    const view = { ...models({ keys: { google: "valid" } }), entries: [known] };
    const listed: WorkModelEntry[] = [{ ...known, roles: ["lead", "page", "light"] }];
    expect(pickerGroups(view, "lead", listed).ready).toEqual([]);
    expect(pickerGroups(view, "page", listed).ready[0]!.entries).toEqual([known]);
    expect(pickerGroups(view, "light", listed).ready[0]!.entries).toEqual([known]);
  });
  test("groups the curated models of usable providers, recommended first, hosted first", () => {
    const view = models({
      keys: { anthropic: "valid", open_ai: "set", google: "invalid" },
      cloud: true,
    });
    const { ready, needsKey } = pickerGroups(view, "lead");
    expect(ready.map((group) => group.provider)).toEqual(["cloud", "anthropic", "open_ai"]);
    expect(ready[1]!.entries.map((item) => item.id)).toEqual([
      "anthropic/claude-opus-5-5",
      "anthropic/claude-sonnet-5-5",
      "anthropic/claude-fable-5-1",
    ]);
    expect(ready[2]!.entries.map((item) => item.id)).toEqual([
      "openai/gpt-6-luna",
      "openai/gpt-6-sol",
      "openai/gpt-6-astra",
    ]);
    // A refused key reads as needing one, not as broken; only the best-known
    // providers ask, and the rest wait in Settings.
    expect(needsKey).toEqual(["google"]);
    // Signed out, hosted models are neither offered nor listed as needing a key.
    const signedOut = pickerGroups(
      { ...view, cloud: { signed_in: false, plan: null, locked: [], required: [] } },
      "lead",
    );
    expect(signedOut.ready.map((group) => group.provider)).toEqual(["anthropic", "open_ai"]);
    expect(signedOut.needsKey).not.toContain("cloud");
    // A plan-locked hosted model stays listed to explain the plan, but a
    // catalog of only locked models cannot run anything.
    const id = view.entries.find((item) => item.model.provider === "cloud")!.id;
    const lockedView = { ...view, cloud: { ...view.cloud, locked: [id] } };
    expect(locked(lockedView, id)).toBe(true);
    expect(usable(lockedView, "cloud")).toBe(false);
    expect(usable(view, "cloud")).toBe(true);
  });

  test("a custom server adds the models it serves", () => {
    const view = models({ base: "http://localhost:11434/v1" });
    const served = [entry("compatible", "qwen4:32b", "qwen4:32b", false)];
    const { ready } = pickerGroups(view, "lead", served);
    expect(ready.map((group) => group.provider)).toEqual(["compatible"]);
    expect(ready[0]!.entries[0]!.display_name).toBe("qwen4:32b");
  });

  test("a custom server is usable once it has an address, keys only when set or valid", () => {
    const view: WorkModelsV1 = models({
      keys: { deep_seek: "invalid" },
      base: "http://localhost:11434/v1",
    });
    expect(usable(view, "compatible")).toBe(true);
    expect(usable(view, "deep_seek")).toBe(false);
    expect(usable(null, "anthropic")).toBe(false);
  });

  test("the trigger drops a family name the mark already says", () => {
    const opus: WorkModelEntry = entry("anthropic", "claude-opus-5-5", "Claude Opus 5.5", true);
    expect(shortName(opus)).toBe("Opus 5.5");
    expect(shortName(entry("open_ai", "gpt-6-sol", "GPT-6 Sol", true))).toBe("GPT-6 Sol");
  });
});

describe("the picker's details", () => {
  test("context windows and prices read the way a model card says them", () => {
    expect(contextSize(1_000_000)).toBe("1M");
    expect(contextSize(1_048_576)).toBe("1M");
    expect(contextSize(1_050_000)).toBe("1M");
    expect(contextSize(200_000)).toBe("200K");
    expect(contextSize(131_072)).toBe("128K");
    expect(dollars(2_000_000)).toBe("$2");
    expect(dollars(1_320_000)).toBe("$1.32");
    expect(dollars(100_000)).toBe("$0.1");
    expect(dollars(10_000)).toBe("$0.01");
    expect(priceTier({ input: 100_000, cached_input: 10_000, output: 500_000 })).toBe(1);
    expect(priceTier({ input: 2_000_000, cached_input: 200_000, output: 10_000_000 })).toBe(2);
    expect(priceTier({ input: 4_000_000, cached_input: 200_000, output: 20_000_000 })).toBe(3);
    expect(priceTier({ input: 10_000_000, cached_input: 1_000_000, output: 50_000_000 })).toBe(4);
    expect(priceTier(null)).toBeNull();
  });

  test("search finds models across providers by name, maker or id", () => {
    const view = models({ keys: { anthropic: "valid", open_ai: "valid" } });
    const { ready } = pickerGroups(view, "lead");
    expect(searchEntries(ready, "sonnet").map((item) => item.id)).toEqual([
      "anthropic/claude-sonnet-5-5",
    ]);
    expect(searchEntries(ready, "openai astra").map((item) => item.id)).toEqual([
      "openai/gpt-6-astra",
    ]);
    expect(searchEntries(ready, "  ")).toEqual([]);
  });

  test("each entry offers the efforts Rust says it takes", () => {
    const view = models({ keys: { deep_seek: "valid" }, lead: "deepseek/deepseek-v4-pro" });
    expect(effortLevels(view, "deepseek/deepseek-v4-pro")).toEqual(["low", "high"]);
    expect(effortLevels(view, "nobody/none")).toEqual([]);
    expect(view.effort).toBe("high");
  });
});
