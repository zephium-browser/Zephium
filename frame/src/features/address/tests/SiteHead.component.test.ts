import { createRawSnippet } from "svelte";
import { expect, test } from "vitest";
import { render } from "vitest-browser-svelte";
import "$styles/global.css";
import SiteHead from "../components/SiteHead.svelte";
import type { SitePage } from "../lib/address-model";

function pageOf(over: Partial<SitePage> = {}): SitePage {
  return {
    url: "https://meet.example/room",
    security: "secure",
    warning: null,
    capture: null,
    privateWindow: false,
    zoom: null,
    standardZoom: 100,
    ...over,
  };
}

const control = createRawSnippet(() => ({
  render: () => `<button type="button" class="control" style="inline-size: 45px">On</button>`,
}));

const box = (element: Element) => element.getBoundingClientRect();

test("a secure page is a green shield, what the control does and the control, on one row", async () => {
  const screen = await render(SiteHead, {
    page: pageOf(),
    label: "Block on this site",
    children: control,
  });
  await expect.element(screen.getByText("Block on this site")).toBeVisible();
  const shield = screen.getByRole("img", { name: "Secure" });
  await expect.element(shield).toHaveAttribute("title", "Secure");
  expect(shield.element().hasAttribute("data-secure")).toBe(true);
  expect(screen.container.textContent).not.toContain("Secure");
  expect(screen.container.textContent).not.toContain("meet.example");

  const head = screen.container.querySelector<HTMLElement>(".head")!;
  const name = screen.getByText("Block on this site").element();
  expect(head.offsetHeight).toBe(34);
  expect(box(shield.element()).right).toBeLessThanOrEqual(box(name).left);
  expect(box(name).right).toBeLessThanOrEqual(box(head.querySelector(".control")!).left);
  expect(screen.container.querySelectorAll("p")).toHaveLength(0);
});

test("an unencrypted page has a red alert shield, and says why under the name", async () => {
  const screen = await render(SiteHead, {
    page: pageOf({ url: "http://meet.example:8080/", security: "insecure" }),
    label: "Block on this site",
    children: control,
  });
  const shield = screen.getByRole("img", { name: "Not secure" });
  await expect.element(shield).toBeVisible();
  expect(shield.element().hasAttribute("data-secure")).toBe(false);
  const status = screen.getByText("Not secure");
  await expect.element(status).toBeVisible();
  expect(box(screen.getByText("Block on this site").element()).bottom).toBeLessThanOrEqual(
    box(status.element()).top,
  );
  expect(screen.container.textContent).not.toContain("Secure");
  expect(screen.container.textContent).not.toContain("meet.example");
});

test("a certificate the person continued past is named, never secure", async () => {
  const screen = await render(SiteHead, {
    page: pageOf({ security: "untrusted" }),
    label: "Block on this site",
  });
  await expect.element(screen.getByText("Certificate not trusted")).toBeVisible();
  await expect.element(screen.getByRole("img", { name: "Certificate not trusted" })).toBeVisible();
  expect(screen.container.textContent).not.toContain("Secure");
});

test("without a name the status takes its place", async () => {
  const screen = await render(SiteHead, { page: pageOf({ security: "insecure" }) });
  await expect.element(screen.getByText("Not secure")).toBeVisible();
  expect(screen.container.querySelectorAll(".status")).toHaveLength(0);
});

test("a page with no site has no head", async () => {
  const screen = await render(SiteHead, { page: null, label: "Block on this site" });
  expect(screen.container.querySelector(".head")).toBeNull();
});

test("a long name takes a second line inside the row rather than leaving it", async () => {
  const screen = await render(SiteHead, {
    page: pageOf(),
    label: "Auf dieser Website blockieren",
    children: control,
  });
  // The default column's panel, less its own padding; then the narrowest.
  for (const width of ["228px", "168px"]) {
    screen.container.style.inlineSize = width;
    const head = screen.container.querySelector<HTMLElement>(".head")!;
    const name = screen.getByText("Auf dieser Website blockieren").element() as HTMLElement;
    await expect.element(screen.getByText("Auf dieser Website blockieren")).toBeVisible();
    expect(head.scrollWidth).toBeLessThanOrEqual(head.clientWidth);
    expect(name.scrollWidth).toBeLessThanOrEqual(name.clientWidth);
    expect(box(name).right).toBeLessThanOrEqual(box(head.querySelector(".control")!).left);
    if (width === "228px") expect(head.offsetHeight).toBeLessThanOrEqual(38);
  }
});
