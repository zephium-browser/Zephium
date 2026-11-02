import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import type { PageRequestView } from "$shared/ipc/bindings";
import PageRequestCard from "../components/PageRequestCard.svelte";

const answers = vi.hoisted(() => vi.fn(async () => ({ outcome: "accepted" })));
const pageAnswers = vi.hoisted(() => vi.fn(async () => ({ accepted: true, operation_id: null })));

vi.mock("$shared/ipc/bindings", async () => {
  const { mockBindings } = await import("$shared/testing/bindings");
  return mockBindings({
    tabsAnswerSignIn: answers as never,
    tabsAnswerPageRequest: pageAnswers as never,
  });
});

afterEach(() => {
  answers.mockClear();
  pageAnswers.mockClear();
});

function signIn(overrides: Partial<Extract<PageRequestView, { kind: "http_auth" }>> = {}) {
  const request: PageRequestView = {
    kind: "http_auth",
    request: 7,
    host: "router.lan",
    insecure: false,
    proxy: false,
    retry: false,
    ...overrides,
  };
  return render(PageRequestCard, { tab: "tab-1", request });
}

test("a sign-in prompt names the host and starts in the username", async () => {
  signIn();
  const card = page.getByRole("alertdialog");
  await expect.element(card).toHaveTextContent("Sign in to router.lan");
  await expect.element(page.getByLabelText("Username")).toHaveFocus();
  await expect.element(page.getByRole("button", { name: "Sign In" })).toBeDisabled();
  await expect.element(card).not.toHaveTextContent("isn't encrypted");
  await expect.element(card).not.toHaveTextContent("Wrong username");
});

test("plain http warns, and a retry says the last attempt was wrong", async () => {
  signIn({ insecure: true, retry: true });
  const card = page.getByRole("alertdialog");
  await expect.element(card).toHaveTextContent("This connection isn't encrypted");
  await expect.element(card).toHaveTextContent("Wrong username or password.");
});

test("a proxy is named as the proxy", async () => {
  signIn({ proxy: true, host: "proxy.corp:3128" });
  await expect
    .element(page.getByRole("alertdialog"))
    .toHaveTextContent("The proxy proxy.corp:3128 asks you to sign in");
});

test("Enter signs in with what was typed and clears the fields", async () => {
  signIn();
  await userEvent.type(page.getByLabelText("Username"), "admin");
  await userEvent.type(page.getByLabelText("Password"), "hunter2{Enter}");
  expect(answers).toHaveBeenCalledExactlyOnceWith("tab-1", 7, {
    username: "admin",
    password: "hunter2",
  });
  await expect.element(page.getByLabelText("Password")).toHaveValue("");
});

test("Enter inside the card never submits the address form around it", async () => {
  const submitted = vi.fn((event: SubmitEvent) => event.preventDefault());
  const form = document.createElement("form");
  form.addEventListener("submit", submitted);
  document.body.append(form);
  render(PageRequestCard, {
    target: form,
    props: {
      tab: "tab-1",
      request: {
        kind: "http_auth",
        request: 3,
        host: "nas.local",
        insecure: false,
        proxy: false,
        retry: false,
      },
    },
  });
  await userEvent.type(page.getByLabelText("Username"), "{Enter}");
  expect(submitted).not.toHaveBeenCalled();
  expect(answers).not.toHaveBeenCalled();
  form.remove();
});

test("Escape and Cancel cancel without credentials", async () => {
  signIn();
  await userEvent.type(page.getByLabelText("Username"), "admin{Escape}");
  expect(answers).toHaveBeenLastCalledWith("tab-1", 7, null);
  await page.getByRole("button", { name: "Cancel" }).click();
  expect(answers).toHaveBeenCalledTimes(2);
  expect(answers).toHaveBeenLastCalledWith("tab-1", 7, null);
});

test("a blocked pop-up can be opened and its site always allowed", async () => {
  render(PageRequestCard, {
    tab: "tab-1",
    request: { kind: "popup", host: "accounts.example", site: "mail.example" },
  });
  await page.getByRole("checkbox", { name: "Always allow pop-ups from mail.example" }).click();
  await page.getByRole("button", { name: "Open" }).click();
  expect(pageAnswers).toHaveBeenLastCalledWith("tab-1", "always_allow");
});

test("a pop-up without an address can still allow its site", async () => {
  render(PageRequestCard, {
    tab: "tab-1",
    request: { kind: "popup", host: null, site: "mail.example" },
  });
  await page.getByRole("checkbox", { name: "Always allow pop-ups from mail.example" }).click();
  await page.getByRole("button", { name: "OK" }).click();
  expect(pageAnswers).toHaveBeenLastCalledWith("tab-1", "always_allow");
});

test("a private tab's blocked pop-up offers no lasting choice", async () => {
  render(PageRequestCard, {
    tab: "tab-1",
    request: { kind: "popup", host: "accounts.example", site: null },
  });
  await expect.element(page.getByRole("checkbox")).not.toBeInTheDocument();
  await page.getByRole("button", { name: "Open" }).click();
  expect(pageAnswers).toHaveBeenLastCalledWith("tab-1", "allow");
});
