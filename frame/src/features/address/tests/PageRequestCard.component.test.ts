import { afterEach, expect, test, vi } from "vitest";
import { render } from "vitest-browser-svelte";
import { page, userEvent } from "vitest/browser";
import type { OperationAdmission, PageRequestView } from "$shared/ipc/bindings";
import PageRequestCard from "../components/PageRequestCard.svelte";

const answers = vi.hoisted(() =>
  vi.fn(async (): Promise<OperationAdmission> => ({ accepted: true, operation_id: null })),
);
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
  return render(PageRequestCard, { tab: "tab-1", serial: 5, request });
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

test("a refused sign-in answer is reported and can be retried", async () => {
  let finish!: (admission: OperationAdmission) => void;
  answers.mockReturnValueOnce(new Promise((resolve) => (finish = resolve)));
  signIn();
  const password = page.getByLabelText("Password");
  await userEvent.type(page.getByLabelText("Username"), "admin");
  await userEvent.type(password, "first{Enter}");
  await userEvent.keyboard("{Escape}");
  expect(answers).toHaveBeenCalledExactlyOnceWith("tab-1", 7, {
    username: "admin",
    password: "first",
  });
  finish({ accepted: false, operation_id: null });
  await expect.element(page.getByRole("alert")).toHaveTextContent("Couldn't apply your answer.");
  await expect.element(password).toHaveValue("");
  await userEvent.type(page.getByLabelText("Username"), "admin");
  await userEvent.type(password, "second{Enter}");
  expect(answers).toHaveBeenCalledTimes(2);
  expect(answers).toHaveBeenLastCalledWith("tab-1", 7, {
    username: "admin",
    password: "second",
  });
  await expect.element(page.getByRole("alert")).not.toBeInTheDocument();
});

test("a lost sign-in cancellation is reported and can be retried", async () => {
  answers.mockRejectedValueOnce(new Error("transport failure"));
  signIn();
  const cancel = page.getByRole("button", { name: "Cancel" });
  await cancel.click();
  await expect.element(page.getByRole("alert")).toHaveTextContent("Couldn't apply your answer.");
  await cancel.click();
  expect(answers).toHaveBeenCalledTimes(2);
  expect(answers).toHaveBeenLastCalledWith("tab-1", 7, null);
  await expect.element(page.getByRole("alert")).not.toBeInTheDocument();
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
      serial: 5,
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
    serial: 5,
    request: { kind: "popup", host: "accounts.example", site: "mail.example" },
  });
  await page.getByRole("checkbox", { name: "Always allow pop-ups from mail.example" }).click();
  await page.getByRole("button", { name: "Open" }).click();
  expect(pageAnswers).toHaveBeenLastCalledWith("tab-1", 5, "always_allow");
});

test("a pop-up without an address can still allow its site", async () => {
  render(PageRequestCard, {
    tab: "tab-1",
    serial: 5,
    request: { kind: "popup", host: null, site: "mail.example" },
  });
  await page.getByRole("checkbox", { name: "Always allow pop-ups from mail.example" }).click();
  await page.getByRole("button", { name: "OK" }).click();
  expect(pageAnswers).toHaveBeenLastCalledWith("tab-1", 5, "always_allow");
});

test("a private tab's blocked pop-up offers no lasting choice", async () => {
  render(PageRequestCard, {
    tab: "tab-1",
    serial: 5,
    request: { kind: "popup", host: "accounts.example", site: null },
  });
  await expect.element(page.getByRole("checkbox")).not.toBeInTheDocument();
  await page.getByRole("button", { name: "Open" }).click();
  expect(pageAnswers).toHaveBeenLastCalledWith("tab-1", 5, "allow");
});

function download(overrides: Partial<Extract<PageRequestView, { kind: "download" }>> = {}) {
  const request: PageRequestView = {
    kind: "download",
    request: 3,
    site: "files.example",
    filename: "report.pdf",
    allowed: false,
    unclicked: false,
    ...overrides,
  };
  return render(PageRequestCard, { tab: "tab-1", serial: 5, request });
}

test("a site's first download asks to allow the site, naming the file", async () => {
  download();
  const card = page.getByRole("alertdialog");
  await expect.element(card).toHaveTextContent("Allow downloads from files.example?");
  await expect.element(card).toHaveTextContent("report.pdf");
  await expect.element(card).not.toHaveTextContent("You didn't click");
  await page.getByRole("button", { name: "Not now" }).click();
  expect(pageAnswers).toHaveBeenCalledExactlyOnceWith("tab-1", 5, "dismiss");
  await page.getByRole("button", { name: "Allow" }).click();
  expect(pageAnswers).toHaveBeenLastCalledWith("tab-1", 5, "allow");
});

test("an unclicked download on an allowed site is confirmed once", async () => {
  download({ allowed: true, unclicked: true });
  const card = page.getByRole("alertdialog");
  await expect.element(card).toHaveTextContent("files.example wants to download a file");
  await expect.element(card).toHaveTextContent("You didn't click anything that downloads.");
  await page.getByRole("button", { name: "Download" }).click();
  expect(pageAnswers).toHaveBeenCalledExactlyOnceWith("tab-1", 5, "allow");
  await expect.element(page.getByRole("button", { name: "Cancel" })).toBeVisible();
});

test("an answer that does not land says so and can be given again", async () => {
  pageAnswers.mockResolvedValueOnce({ accepted: false, operation_id: null });
  render(PageRequestCard, {
    tab: "tab-1",
    serial: 9,
    request: { kind: "external_app", site: "meet.example", scheme: "zoommtg", app: "zoom.us" },
  });
  await page.getByRole("button", { name: "Open" }).click();
  expect(pageAnswers).toHaveBeenLastCalledWith("tab-1", 9, "allow");
  await expect.element(page.getByRole("alert")).toHaveTextContent("Couldn't apply your answer.");
  await page.getByRole("button", { name: "Open" }).click();
  expect(pageAnswers).toHaveBeenCalledTimes(2);
  await expect.element(page.getByRole("alert")).not.toBeInTheDocument();
});

test("AutoFill on a page that is not encrypted asks before filling", async () => {
  render(PageRequestCard, {
    tab: "tab-1",
    serial: 5,
    request: { kind: "insecure_autofill", host: "shop.example" },
  });
  const card = page.getByRole("alertdialog");
  await expect.element(card).toHaveTextContent("This page isn't secure");
  await expect.element(card).toHaveTextContent("shop.example doesn't encrypt its connection");
  await page.getByRole("button", { name: "Fill Anyway" }).click();
  expect(pageAnswers).toHaveBeenCalledExactlyOnceWith("tab-1", 5, "allow");
});

test("Cancel on the AutoFill warning fills nothing", async () => {
  render(PageRequestCard, {
    tab: "tab-1",
    serial: 5,
    request: { kind: "insecure_autofill", host: "shop.example" },
  });
  await page.getByRole("button", { name: "Cancel" }).click();
  expect(pageAnswers).toHaveBeenCalledExactlyOnceWith("tab-1", 5, "dismiss");
});
