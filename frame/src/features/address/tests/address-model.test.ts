import { describe, expect, it } from "vitest";
import { addressSecurity, editingAddress, restingAddress } from "../lib/address-model";

describe("address presentation", () => {
  it("shows only the host while browser chrome is at rest", () => {
    expect(restingAddress("https://docs.example.com:8443/guide?q=1#top")).toBe(
      "docs.example.com:8443",
    );
  });

  it("preserves the complete authoritative URL only for explicit editing", () => {
    const url = "https://example.com/path?q=1";
    expect(editingAddress(url)).toBe(url);
  });

  it("fails closed for absent or invalid authoritative URLs", () => {
    expect(restingAddress(null)).toBe("");
    expect(restingAddress("not a url")).toBe("");
  });

  it("never shows a page under a certificate exception as secure", () => {
    expect(addressSecurity("https://nas.local/")).toBe("secure");
    expect(addressSecurity("https://nas.local/", true)).toBe("untrusted");
    expect(addressSecurity("http://nas.local/", true)).toBe("insecure");
    expect(addressSecurity("about:blank", true)).toBe("none");
  });
});
