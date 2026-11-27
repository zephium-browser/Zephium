import { describe, expect, it } from "vitest";
import { compareVersions, releaseNotesUrl, systemUpdateTarget } from "../versions";

describe("versions", () => {
  it("orders releases, prereleases and build metadata like semver", () => {
    expect(compareVersions("1.0.0", "1.0.0")).toBe(0);
    expect(compareVersions("1.0.1", "1.0.0")).toBe(1);
    expect(compareVersions("1.2.0", "1.10.0")).toBe(-1);
    expect(compareVersions("1.0.0-beta.1", "1.0.0")).toBe(-1);
    expect(compareVersions("1.0.0-beta.2", "1.0.0-beta.10")).toBe(-1);
    expect(compareVersions("1.0.0-beta", "1.0.0-beta.1")).toBe(-1);
    expect(compareVersions("1.0.0-1", "1.0.0-alpha")).toBe(-1);
    expect(compareVersions("1.0.0+7", "1.0.0+8")).toBe(0);
    expect(compareVersions("latest", "1.0.0")).toBeNull();
  });

  it("links a version's entry on the site's changelog", () => {
    expect(releaseNotesUrl("1.0.3")).toBe("https://zephium.app/changelog#v1-0-3");
    expect(releaseNotesUrl("1.0.0-beta.2")).toBe("https://zephium.app/changelog#v1-0-0-beta-2");
    expect(releaseNotesUrl("1.0.0+7")).toBe("https://zephium.app/changelog#v1-0-0");
  });

  it("surfaces only platform updates a person can install", () => {
    expect(systemUpdateTarget([])).toBeNull();
    expect(
      systemUpdateTarget([
        { kind: "review_overdue", update_target: "zephium" },
        { kind: "unreviewed_runtime", update_target: "browser_runtime" },
        { kind: "update_recommended", update_target: "zephium" },
      ]),
    ).toBeNull();
    expect(
      systemUpdateTarget([
        { kind: "review_overdue", update_target: "zephium" },
        { kind: "update_recommended", update_target: "operating_system" },
      ]),
    ).toBe("operating_system");
    expect(
      systemUpdateTarget([{ kind: "update_recommended", update_target: "browser_runtime" }]),
    ).toBe("browser_runtime");
  });
});
