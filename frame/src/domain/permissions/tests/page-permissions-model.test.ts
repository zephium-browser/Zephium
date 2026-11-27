import { describe, expect, it } from "vitest";
import type { PagePermissionPromptView } from "$shared/ipc/bindings";
import {
  PagePermissionPromptProjectionModel,
  initialPagePermissionPrompt,
} from "../page-permissions-model";
import { ZERO_PROJECTION_REVISION } from "$domain/tabs";

function revision(value: number): string {
  return value.toString(16).padStart(32, "0");
}

function prompt(value: number, requestId = "0000000000000001"): PagePermissionPromptView {
  return {
    projection_revision: revision(value),
    prompt: {
      profile_id: "profile-a",
      item_id: "tab-a",
      request_id: requestId,
      origin: "https://media.example",
      kinds: ["camera", "microphone"],
      rememberable: true,
      processing: false,
      in_sidebar: true,
      blocked_by_system: false,
    },
  };
}

describe("page permission prompt projection admission", () => {
  it("starts closed at the global zero revision", () => {
    expect(initialPagePermissionPrompt()).toEqual({
      projection_revision: ZERO_PROJECTION_REVISION,
      prompt: null,
    });
  });

  it("cannot revive stale consent controls after authoritative closure", () => {
    const model = new PagePermissionPromptProjectionModel();
    const current = prompt(3);
    expect(model.apply(current)).toBe(true);
    expect(model.apply(prompt(3, "0000000000000002"))).toBe(false);

    const closed: PagePermissionPromptView = {
      projection_revision: revision(4),
      prompt: null,
    };
    expect(model.apply(closed)).toBe(true);
    expect(model.apply(prompt(2, "0000000000000003"))).toBe(false);
    expect(model.view).toBe(closed);
  });
});
