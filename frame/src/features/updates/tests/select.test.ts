import { describe, expect, it } from "vitest";
import { selectNotices, type NoticeFacts } from "../lib/select";

const facts = (overrides: Partial<NoticeFacts> = {}): NoticeFacts => ({
  status: { state: "upToDate" },
  relaunching: false,
  version: "1.0.1",
  seen: "1.0.1",
  securityDismissed: "",
  advisories: [],
  sessionSetAside: false,
  ...overrides,
});
const behind = [{ kind: "update_recommended", update_target: "operating_system" }] as const;

describe("update notices", () => {
  it("is quiet when there is nothing to say", () => {
    expect(selectNotices(facts())).toEqual({ pill: null, card: null });
  });

  it("offers the relaunch while an update waits, and shows it under way", () => {
    expect(selectNotices(facts({ status: { state: "ready", version: "1.0.2" } })).pill).toEqual({
      kind: "ready",
      version: "1.0.2",
    });
    expect(
      selectNotices(facts({ status: { state: "ready", version: "1.0.2" }, relaunching: true }))
        .pill,
    ).toEqual({ kind: "installing" });
    expect(selectNotices(facts({ status: { state: "installing" } })).pill).toEqual({
      kind: "installing",
    });
    for (const state of ["idle", "checking", "downloading", "failed"] as const)
      expect(selectNotices(facts({ status: { state } })).pill).toBeNull();
  });

  it("announces a new version once, and never on a first run", () => {
    expect(selectNotices(facts({ seen: "1.0.0" })).card).toEqual({
      kind: "updated",
      version: "1.0.1",
    });
    expect(selectNotices(facts({ seen: "" })).card).toBeNull();
    expect(selectNotices(facts({ seen: null })).card).toBeNull();
    expect(selectNotices(facts({ seen: "1.0.0", version: null })).card).toBeNull();
  });

  it("puts a platform security update ahead of the version notice", () => {
    expect(selectNotices(facts({ seen: "1.0.0", advisories: behind })).card).toEqual({
      kind: "security",
      target: "operating_system",
    });
    expect(
      selectNotices(
        facts({ advisories: [{ kind: "update_recommended", update_target: "browser_runtime" }] }),
      ).card,
    ).toEqual({ kind: "security", target: "browser_runtime" });
  });

  it("keeps internal review notices away from people", () => {
    expect(
      selectNotices(
        facts({
          advisories: [
            { kind: "review_overdue", update_target: "zephium" },
            { kind: "unreviewed_runtime", update_target: "operating_system" },
            { kind: "update_recommended", update_target: "zephium" },
          ],
        }),
      ).card,
    ).toBeNull();
  });

  it("brings a dismissed security notice back only with a newer Zephium", () => {
    const at = (securityDismissed: string | null) =>
      selectNotices(facts({ advisories: behind, securityDismissed })).card?.kind ?? null;
    expect(at("1.0.1")).toBeNull();
    expect(at("1.0.1-beta.3")).toBe("security");
    expect(at("1.0.0")).toBe("security");
    expect(at("1.1.0")).toBeNull();
    expect(at(null)).toBeNull();
    expect(
      selectNotices(facts({ advisories: behind, version: "dev", seen: "dev" })).card?.kind,
    ).toBe("security");
    expect(
      selectNotices(
        facts({ advisories: behind, version: "dev", seen: "dev", securityDismissed: "dev" }),
      ).card,
    ).toBeNull();
  });

  it("shows nothing in a build that cannot update", () => {
    expect(
      selectNotices(facts({ status: { state: "unavailable" }, seen: "1.0.0", advisories: behind })),
    ).toEqual({ pill: null, card: null });
  });

  it("says first, in any build, that the last tabs could not be reopened", () => {
    expect(
      selectNotices(facts({ sessionSetAside: true, seen: "1.0.0", advisories: behind })).card,
    ).toEqual({ kind: "session" });
    expect(
      selectNotices(facts({ status: { state: "unavailable" }, sessionSetAside: true })).card,
    ).toEqual({ kind: "session" });
  });
});
