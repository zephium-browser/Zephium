import { beforeEach, expect, test } from "vitest";
import { ask, dismiss, pending } from "../clear-request.svelte";

beforeEach(dismiss);

test("an address that is not a web page has no site data to clear", () => {
  for (const url of ["about:blank", "file:///tmp/a.html", "zephium://settings", "nonsense", ""]) {
    expect(ask(url)).toBe(false);
  }
  expect(ask(null)).toBe(false);
  expect(pending()).toBeNull();
});

test("asking to clear a page's data opens one confirmation for its host until dismissed", () => {
  expect(ask("https://www.example.com/path?x=1")).toBe(true);
  expect(pending()).toEqual({ url: "https://www.example.com/path?x=1", host: "www.example.com" });
  expect(ask("http://localhost:3000/")).toBe(true);
  expect(pending()?.host).toBe("localhost");
  dismiss();
  expect(pending()).toBeNull();
});
