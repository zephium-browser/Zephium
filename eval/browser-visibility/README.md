# Browser visibility comparisons

Before means the visibility preferences are on (their defaults); after means they are off. Each pair uses the same viewport, theme and fixture data on this PR branch. These are screenshots of the real Svelte UI rendered in WebKit on macOS, with mocked native services—not generated mockups or packaged browser captures. Sidebar images are captured directly from the sidebar element to keep the changed area readable.

<details open>
<summary>Settings sidebar — sign-in card</summary>

| Before — shown (default) | After — hidden |
| --- | --- |
| <img src="before-after/settings-before.png" width="260" alt="Settings sidebar — sign-in card, shown"> | <img src="before-after/settings-after.png" width="260" alt="Settings sidebar — sign-in card, hidden"> |

</details>

<details>
<summary>New Tab — sign-in button</summary>

| Before — shown (default) | After — hidden |
| --- | --- |
| <img src="before-after/newtab-before.png" width="460" alt="New Tab — sign-in button, shown"> | <img src="before-after/newtab-after.png" width="460" alt="New Tab — sign-in button, hidden"> |

</details>

<details>
<summary>Models settings — Zephium AI invitation</summary>

| Before — shown (default) | After — hidden |
| --- | --- |
| <img src="before-after/models-before.png" width="480" alt="Models settings — Zephium AI invitation, shown"> | <img src="before-after/models-after.png" width="480" alt="Models settings — Zephium AI invitation, hidden"> |

</details>

<details>
<summary>Work profile menu — sign-in action</summary>

| Before — shown (default) | After — hidden |
| --- | --- |
| <img src="before-after/profile-menu-before.png" width="420" alt="Work profile menu — sign-in action, shown"> | <img src="before-after/profile-menu-after.png" width="420" alt="Work profile menu — sign-in action, hidden"> |

</details>

<details>
<summary>Browse — expanded sidebar</summary>

| Before — shown (default) | After — hidden |
| --- | --- |
| <img src="before-after/browse-before.png" width="240" alt="Browse — expanded sidebar, shown"> | <img src="before-after/browse-after.png" width="240" alt="Browse — expanded sidebar, hidden"> |

</details>

<details>
<summary>Browse — compact rail</summary>

| Before — shown (default) | After — hidden |
| --- | --- |
| <img src="before-after/compact-before.png" width="70" alt="Browse — compact rail, shown"> | <img src="before-after/compact-after.png" width="70" alt="Browse — compact rail, hidden"> |

</details>

<details>
<summary>Work — compact rail</summary>

| Before — shown (default) | After — hidden |
| --- | --- |
| <img src="before-after/work-before.png" width="70" alt="Work — compact rail, shown"> | <img src="before-after/work-after.png" width="70" alt="Work — compact rail, hidden"> |

</details>

<details open>
<summary>Bookmarks — switch beside the panel</summary>

| Before — shown (default) | After — hidden |
| --- | --- |
| <img src="before-after/bookmarks-before.png" width="390" alt="Bookmarks — switch beside the panel, shown"> | <img src="before-after/bookmarks-after.png" width="390" alt="Bookmarks — switch beside the panel, hidden"> |

</details>

<details>
<summary>Bookmarks — narrow window</summary>

| Before — shown (default) | After — hidden |
| --- | --- |
| <img src="before-after/bookmarks-narrow-before.png" width="390" alt="Bookmarks — narrow window, shown"> | <img src="before-after/bookmarks-narrow-after.png" width="390" alt="Bookmarks — narrow window, hidden"> |

</details>

<details>
<summary>Appearance — visibility controls</summary>

| Before — shown (default) | After — hidden |
| --- | --- |
| <img src="before-after/controls-before.png" width="460" alt="Appearance — visibility controls, shown"> | <img src="before-after/controls-after.png" width="460" alt="Appearance — visibility controls, hidden"> |

</details>

<details>
<summary>Appearance — narrow window</summary>

| Before — shown (default) | After — hidden |
| --- | --- |
| <img src="before-after/controls-narrow-before.png" width="350" alt="Appearance — narrow window, shown"> | <img src="before-after/controls-narrow-after.png" width="350" alt="Appearance — narrow window, hidden"> |

</details>

## Reproduce

Use the pinned tools and component-browser setup in CONTRIBUTING.md, then run:

```sh
pnpm --dir frame run test:component \
  src/features/settings/tests/SettingsNavigation.component.test.ts \
  src/features/settings/tests/AiPageLook.component.test.ts \
  src/features/account/tests/AccountMenu.component.test.ts \
  src/app/browser/tests/NewTabAccountLook.component.test.ts \
  src/app/browser/tests/WorkOffLook.component.test.ts \
  src/features/settings/tests/BrowserControls.component.test.ts
```

The screenshot benches write to `target/look/visibility` and `target/look/newtab-account`. The retained images above are unmodified PNG copies. Account names, bookmark entries, model-key states and tab titles are fixtures; no personal profile or real credential is used. The Work comparison covers its sidebar, not the Work canvas. Native window buttons and page views are not rendered by these component fixtures.

Captured on macOS arm64, November 21, 2026. The five updated suites passed all 24 tests; the Browser controls suite passed 3 tests. Typechecking and changed-file ESLint/Prettier checks passed. This screenshot-only follow-up changes no production components or native behavior; it does not add Windows native acceptance evidence.

