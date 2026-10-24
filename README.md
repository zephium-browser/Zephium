<div align="center">
  <img src=".github/assets/logo.png" width="112" height="112" alt="Zephium" />
  <h1>Zephium</h1>

  <p><strong>A fast, fully featured browser and work environment,<br />reimagined for you and your agents.</strong></p>

  <p>
    <a href="https://github.com/zephium-browser/Zephium/releases"><img src="https://img.shields.io/github/v/release/zephium-browser/Zephium?include_prereleases&label=release&color=blue" alt="Latest release" /></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-MPL--2.0-blue" alt="License: MPL-2.0" /></a>
    <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey" alt="Platforms: macOS and Windows" />
    <a href="https://discord.gg/tyveTUyEp7"><img src="https://img.shields.io/badge/Discord-5865F2?logo=discord&logoColor=white" alt="Discord" /></a>
    <a href="https://www.youtube.com/@crynta"><img src="https://img.shields.io/badge/YouTube-FF0000?logo=youtube&logoColor=white" alt="YouTube" /></a>
    <a href="https://x.com/CryntaDev"><img src="https://img.shields.io/badge/x.com-000000?logo=x&logoColor=white" alt="X: @CryntaDev" /></a>
  </p>

  <p>
    <a href="https://zephium.app">Website</a> ·
    <a href="https://zephium.app/docs">Docs</a> ·
    <a href="https://youtu.be/QbSarcWspvw">Launch film</a> ·
    <a href="https://github.com/zephium-browser/Zephium/releases/latest">Download</a>
  </p>
</div>

<details align="center">
  <summary><sub>Read in another language</sub></summary>
  <sub>
    <a href="docs/readme/README.zh-CN.md">简体中文</a> ·
    <a href="docs/readme/README.ja.md">日本語</a> ·
    <a href="docs/readme/README.ko.md">한국어</a> ·
    <a href="docs/readme/README.hi.md">हिन्दी</a> ·
    <a href="docs/readme/README.es.md">Español</a> ·
    <a href="docs/readme/README.pt-BR.md">Português</a> ·
    <a href="docs/readme/README.fr.md">Français</a> ·
    <a href="docs/readme/README.de.md">Deutsch</a> ·
    <a href="docs/readme/README.pl.md">Polski</a> ·
    <a href="docs/readme/README.ru.md">Русский</a> ·
    <a href="docs/readme/README.id.md">Bahasa Indonesia</a>
  </sub>
</details>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset=".github/assets/browse-dark.webp" />
    <img src=".github/assets/browse-light.webp" alt="Zephium's new tab: the window opens onto the sky, with the name, the time and date, and the day's figures along its foot" width="960" />
  </picture>
</p>

<p align="center">
  <strong>Safari's efficiency. Brave's protection. Arc's design.</strong><br />
  And <strong>Work</strong>, a canvas where your agents do real work in plain sight.
</p>

<p align="center"><a href="https://youtu.be/QbSarcWspvw">▶&nbsp; Watch the launch film</a></p>

---

Zephium is an open-source browser built in Rust on your operating system's own
web engine: WebKit on macOS, the engine Safari uses, and WebView2 on Windows. It
is not a fork of Chromium or Firefox. It blocks ads and trackers natively, runs
Chrome extensions, and puts tasks, notes and your time on the web one click
away. The download is about 30 MB.

One switch away is **Work**. Your work already happens in the browser, where
your tabs, logins and history are. Work brings agents there instead of asking
you to move somewhere else, and lets you watch them do it.

## Work

Describe an outcome in your own words: plan a trip, compare vendors, design a
system, fix a bug. Work takes it from there on a canvas, and you see every
step.

<p align="center">
  <img src=".github/assets/work.webp" alt="A whole Work run in Zephium: helpers reading Uptime Kuma, Gatus and OpenStatus on their own sites, a local helper checking the endpoints, then the answer, a comparison table and charts, a diagram, the config and four tasks" width="960" />
</p>
<p align="center"><sub>Asked for a status page, Work reads three monitors on their own sites, checks your endpoints from your machine, builds the pick, and leaves the comparison, the config and the tasks on the canvas.</sub></p>

- **It starts from what you already have.** Work recalls what it knows about
  you, searches your history when it helps, and loads the right skill for the
  job. Each step appears on the canvas as it happens.
- **Helpers work in parallel, in the open.** For a trip, one helper looks for
  flats while another checks visas and a third compares flights. They browse
  live pages you can watch, and any source opens in a pane beside the canvas.
- **Results stay on the canvas.** Comparisons, tables, charts, diagrams, plans,
  code and documents are laid out where you can read them. They are not lost in
  a chat scroll.
- **A plan becomes your day.** One click turns each step of a plan into a task
  linked back to the Work. Anything worth keeping can be saved as a note.
- **It reaches past the browser.** Work reads and edits the folders you grant,
  runs commands, and hands larger coding jobs to Claude Code or Codex. It
  connects to MCP servers such as Linear, Notion, Sentry, Stripe and Figma, and
  to command-line tools such as `gh`.
- **You stay in charge.** Anything that posts, merges or changes something
  waits for your OK.
- **Bring any model.** Use your own keys for Anthropic, OpenAI, Google Gemini,
  xAI, Mistral, DeepSeek, Groq, Cerebras or OpenRouter, or run a model locally
  through Ollama, LM Studio or any OpenAI-compatible server. Keys live in your
  system keychain. Or sign in to Zephium AI and skip the keys: it picks the
  right model for each step, and you start with free credits
  ([pricing](https://zephium.app/pricing)).

Work ships with 25 skills, including trip planning, research, compare and
choose, plan my day, fix a bug and weekly status. You can write your own.

## Ask Zephium

An assistant in the sidebar, on `⌘ E` (`Ctrl E` on Windows). It reads the page
you are on and answers about it, points at what is worth your time, and opens
what you choose in new tabs. Ask it to take a few steps on the page, or to
restyle a site you visit every day, and it does it in place. When a job
outgrows a chat, **Continue in Work** carries it to the canvas, with the page
along.

<p align="center">
  <img src=".github/assets/ask.webp" alt="Ask Zephium beside Hacker News: three stories picked, opened in new tabs, and the front page re-set as calm cards" width="960" />
</p>
<p align="center"><sub>Asked what is worth reading, it picks three stories, opens them, and re-sets the front page as calm cards.</sub></p>

## Browse

### Fast and light

- Native WebKit on macOS and WebView2 on Windows, so pages run on the engine
  your system already keeps up to date.
- Inactive tabs go to sleep and wake when you return, so memory stays low with
  many tabs open.
- A native ad and tracker blocker, on by default, built on Brave's
  [adblock-rust](https://github.com/brave/adblock-rust) with EasyList and
  EasyPrivacy. Requests are stopped before a page can make them, and you can
  hide anything else on a page with a click.

### Designed to live in

- A new tab that opens onto the sky, by day or by night with your theme, with
  your time, your focus and the day's tasks along its foot.
- Vertical tabs in a quiet sidebar, with spaces, profiles, pinned tabs, folders
  and split view.
- Liquid Glass on macOS 26 and Mica on Windows 11.
- A launcher on `⌘ ⇧ Space` (`Ctrl Shift Space` on Windows) reaches tabs,
  history, notes and commands from anywhere on your desktop. Anything you type
  there can become a task.
- Every keyboard shortcut can be changed.

### Extensions

Install extensions straight from the Chrome Web Store. Twenty popular ones are
verified to work well in Zephium and are a click away in the extension manager,
including 1Password, Bitwarden, Grammarly, DeepL, Dark Reader, Vimium,
SponsorBlock, Raindrop.io, Notion Web Clipper and Refined GitHub.

### Bring everything with you

The welcome flow imports from Chrome, Safari, Arc, Zen, Firefox, Brave and
Edge.

## Built in

<table>
  <tr>
    <td width="33%" align="center"><img src=".github/assets/tasks.webp" alt="Tasks: today, tomorrow and upcoming, with a task linked to the page it came from" /></td>
    <td width="33%" align="center"><img src=".github/assets/time.webp" alt="Time: 3 hours 24 minutes on the web today, a 25-minute focus round and time per site" /></td>
    <td width="33%" align="center"><img src=".github/assets/notes.webp" alt="Notes: a Markdown release checklist with code, beside the page" /></td>
  </tr>
  <tr>
    <td valign="top"><strong>Tasks</strong><br /><sub>Write them the way you would say them, like "call Anna tomorrow at 3". Lists, priorities, deadlines, subtasks, and the page you were on stays linked.</sub></td>
    <td valign="top"><strong>Time</strong><br /><sub>See where your time on the web goes, counted on this device only. Start a focus round and the sites you choose stay shut until the break.</sub></td>
    <td valign="top"><strong>Notes</strong><br /><sub>Notes beside the page, each a Markdown file you own. Edits from other apps show up in Zephium.</sub></td>
  </tr>
</table>

## Private by default

- **No telemetry.** Zephium doesn't collect or send anything about how you
  browse.
- **No account needed.** History, tasks, notes, memory and time live on your
  device. An account is only for Zephium AI, and until you sign in, Zephium
  never contacts our servers.
- **Zephium AI keeps no content.** Requests pass through to the model that
  answers them. We count usage for your plan, never store prompts, answers or
  pages, and never see your card.
- **Private windows keep nothing** once they close.

## Download

| Platform | Architecture | Installer | Requires |
| -------- | ------------ | --------- | -------- |
| macOS | Apple Silicon | [`Zephium-macOS-arm64.dmg`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-macOS-arm64.dmg) | macOS Sonoma 14 or later |
| Windows | x64 | [`Zephium-Windows-x64-setup.exe`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-Windows-x64-setup.exe) | Windows 10 or 11 |

Windows builds are not code-signed yet, so SmartScreen may show "Windows
protected your PC". Choose **More info**, then **Run anyway**. Signing is on
the way.

Zephium downloads updates in the background and installs them when you choose
**Relaunch to update**. All releases are on the
[releases page](https://github.com/zephium-browser/Zephium/releases).

## What's next

- Linux and Intel Macs.
- Code-signed Windows builds.
- More extensions verified to work, and native extensions built into Zephium.
- And much more.

## Build from source

Prerequisites:

- [Rust](https://rustup.rs) through `rustup`. The toolchain pinned in
  [`rust-toolchain.toml`](rust-toolchain.toml) installs itself on first use.
- Node.js, at the version in [`.node-version`](.node-version).
- pnpm, through Corepack: `corepack enable`.
- The [Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your
  platform: Xcode Command Line Tools on macOS, or the Microsoft C++ Build Tools
  and WebView2 on Windows.

```sh
git clone https://github.com/zephium-browser/Zephium.git
cd Zephium
pnpm install --frozen-lockfile
pnpm dev
```

Development builds use their own `app.zephium.dev` profile, so they do not
touch the data of an installed Zephium. Linux is not supported yet. See
[CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

## Architecture

- Untrusted pages run in raw native WebViews, apart from the privileged Svelte
  interface that draws the browser.
- A Rust core owns tabs, storage, the blocker, extensions and agents.
- Each profile has its own isolated website data and Zephium-owned history,
  session and favicon data.
- The network blocker is native and built on Brave's
  [adblock-rust](https://github.com/brave/adblock-rust).
- Narrow in-tree adapters for Tauri and Wry handle storage policy, WebView
  construction, callbacks and teardown.

Read more in [docs/architecture.md](docs/architecture.md) and
[docs/security-model.md](docs/security-model.md).

## Contributing

Zephium is maintained by one person with a clear product direction, so please
discuss larger changes before writing them. [CONTRIBUTING.md](CONTRIBUTING.md)
explains what gets merged and how. Questions and ideas are welcome on
[Discord](https://discord.gg/tyveTUyEp7).

## Security

Please report vulnerabilities privately, not in a public issue. See
[SECURITY.md](SECURITY.md).

## License

Zephium is licensed under the [Mozilla Public License 2.0](LICENSE).

The bundled EasyList and EasyPrivacy filter lists are used under
[CC BY-SA 3.0](assets/blocker-seed/v1/LICENSE-CC-BY-SA-3.0.txt); see
[the notice](assets/blocker-seed/v1/NOTICE) for attribution.

The Zephium name and logo are not licensed under the MPL.

## Acknowledgements

Zephium stands on [Tauri](https://tauri.app), [Wry](https://github.com/tauri-apps/wry),
Brave's [adblock-rust](https://github.com/brave/adblock-rust) and
[EasyList](https://easylist.to).
