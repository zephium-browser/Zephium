import { expect, inject, test } from "vitest";

declare module "vitest" {
  export interface ProvidedContext {
    privilegedPermissionsPolicy: string;
  }
}

test.skipIf(!("featurePolicy" in document))(
  "the shipping clipboard policy allows same-origin writes and denies reads",
  async () => {
    const directives = new Map(
      inject("privilegedPermissionsPolicy")
        .split(", ")
        .map((directive) => directive.split("=") as [string, string]),
    );
    const frame = document.createElement("iframe");
    // Exercise the shipping allowlists through the equivalent container policy.
    frame.allow = ["clipboard-write", "clipboard-read"]
      .map((feature) => {
        const allowlist = directives.get(feature);
        expect(allowlist).toBeDefined();
        return `${feature} ${allowlist === "(self)" ? "'self'" : "'none'"}`;
      })
      .join("; ");
    const loaded = new Promise<void>((resolve) => (frame.onload = () => resolve()));
    frame.srcdoc = "<!doctype html><title>Clipboard policy fixture</title>";
    document.body.append(frame);
    try {
      await loaded;
      const policy = (
        frame.contentDocument as Document & {
          featurePolicy: { allowsFeature: (feature: string) => boolean };
        }
      ).featurePolicy;
      expect(policy.allowsFeature("clipboard-write")).toBe(true);
      expect(policy.allowsFeature("clipboard-read")).toBe(false);
      expect(directives.get("clipboard-write")).toBe("(self)");
      expect(directives.get("clipboard-read")).toBe("()");
    } finally {
      frame.remove();
    }
  },
);
