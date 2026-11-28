row opens its name to be typed over", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  await userEvent.click(screen.getByRole("menuitemradio", { name: "Studio" }), {
    button: "right",
  });
  const name = screen.getByRole("textbox", { name: "Name" });
  await expect.element(name).toHaveValue("Studio");
  await expect.element(name).toHaveFocus();
});

test("a space changes as it is edited, and Done keeps it", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  await screen.getByRole("button", { name: "Edit Studio" }).click();
  const look = {
    icon: { kind: "emoji", text: "🎨" },
    color: 4,
    theme: { kind: "preset", id: "nord" },
  };
  await screen.getByRole("textbox", { name: "Name" }).fill("Atelier");
  await expect.poll(() => native.update.mock.calls.at(-1)).toEqual(["studio", "Atelier", look]);
  await screen.getByRole("radio", { name: "Symbol 4" }).click();
  expect(native.update).toHaveBeenLastCalledWith("studio", "Atelier", {
    ...look,
    icon: { kind: "symbol", index: 3 },
  });
  await screen.getByRole("radio", { name: "No icon" }).click();
  await screen.getByRole("radio", { name: "Color 7" }).click();
  await screen.getByRole("radio", { name: "None", exact: true }).click();
  await expect
    .poll(() => native.update.mock.calls.at(-1))
    .toEqual(["studio", "Atelier", { icon: null, color: 6, theme: null }]);
  await screen.getByRole("button", { name: "Done" }).click();
  await expect
    .element(screen.getByRole("button", { name: "Spaces — Home" }))
    .toHaveAttribute("aria-expanded", "false");
});

test("Escape puts the space back as it was", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  await screen.getByRole("button", { name: "Edit Studio" }).click();
  await screen.getByRole("radio", { name: "Color 7" }).click();
  await screen.getByRole("radio", { name: "No icon" }).click();
  await userEvent.keyboard("{Escape}");
  await expect
    .poll(() => native.update.mock.calls.at(-1))
    .toEqual([
      "studio",
      "Studio",
      { icon: { kind: "emoji", text: "🎨" }, color: 4, theme: { kind: "preset", id: "nord" } },
    ]);
  // Opened from the list, so Escape steps back to it.
  await expect.element(screen.getByRole("menuitemradio").first()).toBeVisible();
});

test("an IME Escape leaves the space editor open", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  await userEvent.click(screen.getByRole("menuitemradio", { name: "Studio" }), {
    button: "right",
  });
  const name = screen.getByRole("textbox", { name: "Name" });
  await expect.element(name).toHaveFocus();

  const escape = new KeyboardEvent("keydown", {
    key: "Escape",
    keyCode: 229,
    bubbles: true,
    cancelable: true,
  });
  name.element().dispatchEvent(escape);
  expect(escape.defaultPrevented).toBe(false);
  await expect.element(name).toHaveFocus();
  expect(native.update).not.toHaveBeenCalled();

  await userEvent.keyboard("{Escape}");
  await expect.element(screen.getByRole("menuitemradio", { name: "Studio" })).toBeVisible();
});

test("deleting asks first and says how many tabs will close", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  await screen.getByRole("button", { name: "Edit Studio" }).click();
  await screen.getByRole("button", { name: "Delete…" }).click();
  await expect.element(screen.getByText("Its 3 tabs will close.")).toBeVisible();
  expect(native.remove).not.toHaveBeenCalled();
  await screen.getByRole("button", { name: "Delete Space" }).click();
  expect(native.remove).toHaveBeenCalledWith("studio");
});

test("New Space makes one at once, with no icon, and opens it to be named", async () => {
  const screen = await mount();
  await screen.getByRole("button", { name: "Spaces — Home" }).click();
  await screen.getByRole("menuitem", { name: "New Space" }).click();
  expect(native.create).toHaveBeenCalledWith("", { icon: null, color: 0, theme: null });
  project({
    spaces: [HOME, STUDIO, spaceFixture("made", "Space 3", { tabs: 1 })],
    active_space_id: "made",
  });
  const name = screen.getByRole("textbox", { name: "Name" });
  await expect.element(name).toHaveValue("Space 3");
  await expect.element(name).toHaveFocus();
  await userEvent.keyboard("Reading{Enter}");
  await expect.poll(() => native.update.mock.calls.at(-1)?.[1]).toBe("Reading");

  emitNativeEvent("uiCommand", "space.edit");
  await expect.element(screen.getByRole("textbox", { name: "Name" })).toHaveValue("Space 3");
});

test("a space with nothing listed draws no empty state; the row's plus still opens a tab", async () => {
  const screen = await mount(true);
  await expect.element(screen.getByRole("button", { name: "Spaces — Home" })).toBeVisible();
  expect(screen.container.textContent).not.toContain("Nothing open");
  await screen.getByRole("button", { name: "New tab", exact: true }).click();
  expect(native.open).toHaveBeenCalledOnce();
});

const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(undefined)));
const sheetOf = (screen: { container: HTMLElement }) =>
  screen.container.querySelector<HTMLElement>(".space-sheet")!;
const swipe = (target: HTMLElement, deltaX: number, count = 6, deltaY = 0, natural = true) => {
  for (let index = 0; index < count; index += 1) {
    const wheel = new WheelEvent("wheel", { deltaX, deltaY, bubbles: true });
    Object.defineProperty(wheel, "webkitDirectionInvertedFromDevice", { value: natural });
    target.dispatchEvent(wheel);
  }
};

test("a sideways swipe moves the spaces as one sheet and settles on the next one", async () => {
  const screen = await mount();
  const list = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  const sheet = sheetOf(screen);
  // Fingers to the right on the first space: nothing to go to, so the sheet
  // resists and comes back.
  swipe(list, -6, 4);
  await frame();
  expect(native.step).not.toHaveBeenCalled();
  expect(sheet.style.transform).toMatch(/^translate3d\(\d/);
  await expect.poll(() => sheet.style.transform, { timeout: 2000 }).toBe("");
  await expect.poll(() => screen.container.querySelector(".peek")).toBeNull();

  // Mostly vertical: not ours.
  swipe(list, 3, 6, 20);
  await frame();
  expect(sheet.style.transform).toBe("");
  await new Promise((resolve) => setTimeout(resolve, 200));

  // Fingers to the left: the next space is already there, arriving from the edge.
  swipe(list, 30);
  expect(native.step).toHaveBeenCalledExactlyOnceWith(1);
  await frame();
  await frame();
  expect(sheet.style.transform).toMatch(/^translate3d\(-/);
  const next = screen.container.querySelector<HTMLElement>('.peek[data-sheet="next"]');
  expect(next).not.toBeNull();
  // The one being left steps back; the one arriving is brighter than it.
  expect(Number.parseFloat(list.style.opacity)).toBeLessThan(1);

  // Native has moved: the sheet carries on from where it is and rests.
  project({ spaces: [HOME, STUDIO], active_space_id: "studio" });
  await expect.poll(() => sheet.style.transform, { timeout: 2000 }).toBe("");
  expect(list.style.opacity).toBe("");
  await expect.poll(() => screen.container.querySelector(".peek")).toBeNull();
});

test("a neighbour the window has shown is drawn as it was, its own tab lit", async () => {
  const screen = await mount();
  const list = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  const studioTabs = [
    tabFixture({ id: "s1", title: "Moodboard" }),
    tabFixture({ id: "s2", title: "Palette" }),
  ];
  const node = (id: string): SidebarNodeView => ({
    id,
    parent_id: null,
    section: "today",
    kind: { type: "tab", tab_id: id },
  });
  project({
    active_space_id: "studio",
    nodes: studioTabs.map((tab) => node(tab.id)),
    tabs: studioTabs,
    active: "s2",
  });
  await frame();
  project({});
  await frame();
  swipe(list, 30);
  await frame();
  await frame();
  const next = screen.container.querySelector<HTMLElement>('.peek[data-sheet="next"]')!;
  expect(next.textContent).toContain("Moodboard");
  expect(next.querySelector('[data-selected="true"]')?.textContent).toContain("Palette");
  // A drawn copy is never a tab to native: it carries none of its markers.
  expect(next.querySelector("[data-zephium-tab-id]")).toBeNull();
  expect(next.inert).toBe(true);
});

test("a neighbour holding no tab peeks as empty until the window opens a tab in it", async () => {
  const screen = await mount();
  const list = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  project({ spaces: [HOME, spaceFixture("quiet", "Quiet", { tabs: 0 })] });
  await frame();
  swipe(list, 30);
  await frame();
  await frame();
  const next = screen.container.querySelector<HTMLElement>('.peek[data-sheet="next"]')!;
  expect(next.textContent).toContain("Nothing open in Quiet");
  expect(next.querySelector("button")).toBeNull();
});

test("a swipe the native side refuses brings the sheet back", async () => {
  native.step.mockImplementationOnce(async () => ({ accepted: false, operation_id: null }));
  const screen = await mount();
  const list = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  swipe(list, 30);
  await frame();
  await expect.poll(() => sheetOf(screen).style.transform, { timeout: 2000 }).toBe("");
});

test("reduced motion admits only one swipe until the space arrives", async () => {
  const screen = await mount();
  const root = document.documentElement;
  const previous = root.dataset.reduceMotion;
  root.dataset.reduceMotion = "true";
  try {
    const list = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
    swipe(list, 30);
    // The wheel fallback can recognize a sharp increase as a fresh gesture.
    swipe(list, 100);
    expect(native.step).toHaveBeenCalledExactlyOnceWith(1);
    expect(sheetOf(screen).style.transform).toBe("");

    project({ active_space_id: "studio" });
    await frame();
    swipe(list, -100);
    expect(native.step.mock.calls).toEqual([[1], [-1]]);
    expect(sheetOf(screen).style.transform).toBe("");
  } finally {
    if (previous === undefined) delete root.dataset.reduceMotion;
    else root.dataset.reduceMotion = previous;
  }
});

test("a space something else switched to slides in without a sheet", async () => {
  const screen = await mount();
  project({ active_space_id: "studio" });
  await frame();
  expect(sheetOf(screen).style.transform).toMatch(/^translate3d\(/);
  expect(screen.container.querySelector(".peek")).toBeNull();
  await expect.poll(() => sheetOf(screen).style.transform, { timeout: 2000 }).toBe("");
});

test("the fingers decide the direction whichever way the wheel is set to run", async () => {
  const screen = await mount();
  const list = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  swipe(list, -30, 6, 0, false);
  expect(native.step).toHaveBeenCalledExactlyOnceWith(1);
});

test("a private window has one space and no switcher", async () => {
  const screen = await mount();
  project({ profile: { id: "x", name: "Private", kind: "incognito" }, spaces: [HOME] });
  await expect.element(screen.getByRole("button", { name: "New tab" })).toBeVisible();
  expect(screen.container.querySelector(".space-bar")).toBeNull();
});

async function mountPrivate() {
  await page.viewport(420, 760);
  await tabs.init();
  await uiCommands.init();
  const open = ["a", "b", "c"].map((id) => tabFixture({ id, title: `Tab ${id}` }));
  const node = (id: string, section: "pinned" | "today"): SidebarNodeView => ({
    id,
    parent_id: null,
    section,
    kind: { type: "tab", tab_id: id },
  });
  project({
    profile: { id: "x", name: "Private", kind: "incognito" },
    spaces: [],
    active_space_id: null,
    nodes: [node("a", "pinned"), node("b", "today"), node("c", "today")],
    tabs: open,
    active: "b",
  });
  flushSync();
  const tree = sidebarTree(tabs.sidebarNodes(), tabs.tabs());
  return render(SidebarBody, {
    pinned: tree.pinned,
    today: tree.today,
    splitting: false,
    onSelect: () => {},
  });
}

test("without spaces New tab follows the last row, with pinned tabs still above the rule", async () => {
  const screen = await mountPrivate();
  screen.container.style.cssText = "display:flex;flex-direction:column;block-size:600px";
  const scroller = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  const button = screen.getByRole("button", { name: "New tab", exact: true }).element();
  expect(scroller.contains(button)).toBe(true);
  expect(button.closest(".new-tab-slot")?.parentElement).toBe(scroller);
  expect(screen.container.querySelector(".space-bar")).toBeNull();

  const sections = [...scroller.children]
    .filter((child) => child.classList.contains("tab-list") || child.classList.contains("tab-divider"))
    .map((child) => (child.classList.contains("tab-divider") ? "divider" : child.getAttribute("aria-label")));
  expect(sections).toEqual(["Pinned tabs", "divider", "Open tabs"]);
  const rows = [...scroller.querySelectorAll<HTMLElement>("[data-zephium-tab-id]")];
  expect(rows.map((row) => row.dataset.zephiumTabId)).toEqual(["a", "b", "c"]);
  expect(button.getBoundingClientRect().top).toBeGreaterThanOrEqual(
    rows.at(-1)!.getBoundingClientRect().bottom,
  );

  await button.click();
  expect(native.open).toHaveBeenCalledOnce();
});

test("dropping a tab in the room below the last row appends it in a private window", async () => {
  const screen = await mountPrivate();
  screen.container.style.cssText = "display:flex;flex-direction:column;block-size:600px";
  const scroller = screen.container.querySelector<HTMLElement>(".tab-scroller")!;
  const handle = screen.container.querySelector<HTMLElement>(
    '.browse-tab[data-zephium-tab-id="b"] .tab-open',
  )!;
  const from = handle.getBoundingClientRect();
  const init = { bubbles: true, pointerId: 1, button: 0, isPrimary: true };
  const start = { x: from.left + 20, y: from.top + from.height / 2 };
  handle.dispatchEvent(new PointerEvent("pointerdown", { ...init, clientX: start.x, clientY: start.y }));
  handle.dispatchEvent(
    new PointerEvent("pointermove", { ...init, clientX: start.x, clientY: start.y + 8 }),
  );
  const room = scroller.getBoundingClientRect();
  const to = { x: room.left + 40, y: room.bottom - 8 };
  handle.dispatchEvent(new PointerEvent("pointermove", { ...init, clientX: to.x, clientY: to.y }));
  await frame();
  handle.dispatchEvent(new PointerEvent("pointerup", { ...init, clientX: to.x, clientY: to.y }));
  await expect.poll(() => native.setEssential.mock.calls.length).toBe(1);
  expect(native.setEssential).toHaveBeenCalledExactlyOnceWith("b", false, null);
  expect(screen.container.querySelectorAll(".tab-divider")).toHaveLength(1);
});
