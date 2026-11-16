import { describe, expect, it } from "vitest";
import { acceptPanelState } from "../lib/panel-model";
import type { PanelState } from "$shared/ipc/bindings";
const snapshot = (revision: string): PanelState => ({
  switcher: null,
  window_id: "window",
  revision,
  session_id: revision,
  visible: true,
  profile_id: "p",
  profile_name: "Personal",
  space_id: "s",
  error: false,
  seed: null,
  seed_tab: null,
  corner_radius: 20,
});
describe("panel presentation and search identity", () => {
  it("rejects stale and malformed panel presentations", () => {
    expect(acceptPanelState(snapshot("0000000000000002"), snapshot("0000000000000001"))).toBe(
      false,
    );
    expect(acceptPanelState(null, snapshot("invalid"))).toBe(false);
    expect(acceptPanelState(null, snapshot("0000000000000001"))).toBe(true);
  });
});
