<div align="center">
  <img src="../../.github/assets/logo.png" width="112" height="112" alt="Zephium" />
  <h1>Zephium</h1>

  <p><strong>快速、功能完备的浏览器与工作环境，<br />为你和你的智能体重新构想。</strong></p>

  <p>
    <a href="https://github.com/zephium-browser/Zephium/releases"><img src="https://img.shields.io/github/v/release/zephium-browser/Zephium?include_prereleases&label=release&color=blue" alt="最新版本" /></a>
    <a href="../../LICENSE"><img src="https://img.shields.io/badge/license-MPL--2.0-blue" alt="许可证：MPL-2.0" /></a>
    <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey" alt="支持平台：macOS 和 Windows" />
    <a href="https://discord.gg/tyveTUyEp7"><img src="https://img.shields.io/badge/Discord-5865F2?logo=discord&logoColor=white" alt="Discord" /></a>
    <a href="https://www.youtube.com/@crynta"><img src="https://img.shields.io/badge/YouTube-FF0000?logo=youtube&logoColor=white" alt="YouTube" /></a>
    <a href="https://x.com/CryntaDev"><img src="https://img.shields.io/badge/x.com-000000?logo=x&logoColor=white" alt="X: @CryntaDev" /></a>
  </p>

  <p>
    <a href="https://zephium.app">官网</a> ·
    <a href="https://zephium.app/docs">文档</a> ·
    <a href="https://youtu.be/QbSarcWspvw">发布影片</a> ·
    <a href="https://github.com/zephium-browser/Zephium/releases/latest">下载</a>
  </p>
</div>

<details align="center">
  <summary><sub>选择其他语言</sub></summary>
  <sub>
    <a href="../../README.md">English</a> ·
    <a href="README.ja.md">日本語</a> ·
    <a href="README.ko.md">한국어</a> ·
    <a href="README.hi.md">हिन्दी</a> ·
    <a href="README.es.md">Español</a> ·
    <a href="README.pt-BR.md">Português</a> ·
    <a href="README.fr.md">Français</a> ·
    <a href="README.de.md">Deutsch</a> ·
    <a href="README.pl.md">Polski</a> ·
    <a href="README.ru.md">Русский</a> ·
    <a href="README.id.md">Bahasa Indonesia</a>
  </sub>
</details>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../../.github/assets/browse-dark.webp" />
    <img src="../../.github/assets/browse-light.webp" alt="Zephium 的新标签页：窗口朝向天空打开，页脚处是名称、时间与日期，以及当天的数据" width="960" />
  </picture>
</p>

<p align="center">
  <strong>Safari 的高效，Brave 的防护，Arc 的设计。</strong><br />
  还有 <strong>Work</strong>：一块画布，让你的智能体在你眼前完成真正的工作。
</p>

<p align="center"><a href="https://youtu.be/QbSarcWspvw">▶&nbsp; 观看发布影片</a></p>

---

Zephium 是一款开源浏览器，用 Rust 构建，基于操作系统自带的网页引擎：macOS 上是 WebKit（Safari 所用的引擎），Windows 上是 WebView2。它不是 Chromium 或 Firefox 的分支。它原生拦截广告和跟踪器，可运行 Chrome 扩展，并让任务、笔记和你的上网时间一键可达。安装包约 30 MB。

只需一个开关，就能进入 **Work**。你的工作本来就在浏览器里进行，标签页、登录状态和历史记录都在这里。Work 把智能体带到这里，而不是让你转去别处，并且让你亲眼看着它们工作。

## Work

用自己的话描述想要的结果：规划一次旅行、比较供应商、设计一个系统、修复一个 bug。Work 会在画布上接手，你能看到每一个步骤。

<p align="center">
  <img src="../../.github/assets/work.webp" alt="Zephium 中完整的一次 Work 运行：助手在各自的官网上阅读 Uptime Kuma、Gatus 和 OpenStatus，一个本地助手检查各个端点，随后是答案、对比表格和图表、一张示意图、配置文件和四项任务" width="960" />
</p>
<p align="center"><sub>被要求做一个状态页时，Work 会在三款监控工具各自的官网上阅读资料，用你自己的机器检查你的端点，给出最终选择，并把对比、配置和任务留在画布上。</sub></p>

- **从你已有的东西开始。** Work 会回忆它对你的了解，在有帮助时搜索你的历史记录，并为任务加载合适的技能。每一步发生时都会显示在画布上。
- **助手并行工作，一切公开可见。** 规划旅行时，一个助手找房子，另一个查签证，第三个比较航班。它们浏览的是你能亲眼看到的实时页面，任何来源都可以在画布旁的面板中打开。
- **结果留在画布上。** 对比、表格、图表、示意图、计划、代码和文档都排布在方便阅读的位置，不会淹没在滚动的聊天记录里。
- **计划变成你的一天。** 一次点击，就能把计划中的每一步变成任务，并链接回对应的 Work。任何值得保留的内容都可以存为笔记。
- **能力延伸到浏览器之外。** Work 可以读取和编辑你授权的文件夹，运行命令，并把较大的编码任务交给 Claude Code 或 Codex。它能连接 Linear、Notion、Sentry、Stripe、Figma 等 MCP 服务器，以及 `gh` 这类命令行工具。
- **决定权始终在你。** 任何会发布、合并或更改内容的操作，都会等你确认。
- **随意选择模型。** 使用你自己的密钥接入 Anthropic、OpenAI、Google Gemini、DeepSeek 或 OpenRouter，或通过 Ollama、LM Studio 或任何兼容 OpenAI 的服务器在本地运行模型。密钥保存在系统钥匙串中。或者登录 Zephium AI，免去密钥：它会为每一步选出合适的模型，开始时还有免费额度（[定价](https://zephium.app/pricing)）。

Work 内置 25 项技能，包括旅行规划、调研、对比与选择、规划我的一天、修复 bug 和每周状态汇报。你也可以编写自己的技能。

## Ask Zephium

侧边栏里的助手，快捷键是 `⌘ E`（Windows 上为 `Ctrl E`）。它会读取你正在看的页面并回答相关问题，指出哪些内容值得你花时间，并把你选中的内容在新标签页中打开。你可以让它在页面上执行几个步骤，或者为你每天都会访问的网站换一套样式，它会就地完成。当任务超出一次聊天能承载的范围时，**Continue in Work** 会把它连同当前页面一起带到画布上。

<p align="center">
  <img src="../../.github/assets/ask.webp" alt="Ask Zephium 位于 Hacker News 旁：挑出三篇文章，在新标签页中打开，并把首页重新排成清爽的卡片" width="960" />
</p>
<p align="center"><sub>被问到什么值得读时，它挑出三篇文章，把它们打开，并把首页重新排成清爽的卡片。</sub></p>

## Browse

### 快速轻盈

- macOS 上使用原生 WebKit，Windows 上使用 WebView2，网页运行在系统本就持续更新的引擎上。
- 不活跃的标签页会进入休眠，你回来时再唤醒，所以即使打开很多标签页，内存占用也很低。
- 原生的广告和跟踪器拦截器，默认开启，基于 Brave 的 [adblock-rust](https://github.com/brave/adblock-rust)，搭配 EasyList 和 EasyPrivacy。请求在页面发出之前就被拦下，页面上的其他内容，点一下就能隐藏。

### 为长期使用而设计

- 新标签页朝向天空打开，随你的主题呈现白天或夜晚，页脚处显示时间、专注状态和当天的任务。
- 安静的侧边栏中的垂直标签页，支持空间、配置文件、固定标签页、文件夹和分屏视图。
- macOS 26 上的 Liquid Glass，Windows 11 上的 Mica。
- 启动器快捷键为 `⌘ ⇧ Space`（Windows 上为 `Ctrl Shift Space`），可在桌面任何位置访问标签页、历史记录、笔记和命令。在那里输入的任何内容都可以变成任务。
- 每一个键盘快捷键都可以修改。

### 扩展

可直接从 Chrome 网上应用店安装扩展。有二十款热门扩展已验证能在 Zephium 中良好运行，在扩展管理器中一键即可安装，包括 1Password、Bitwarden、Grammarly、DeepL、Dark Reader、Vimium、SponsorBlock、Raindrop.io、Notion Web Clipper 和 Refined GitHub。

### 带上你的一切

欢迎向导支持从 Chrome、Safari、Arc、Zen、Firefox、Brave 和 Edge 导入。

## 内置功能

<table>
  <tr>
    <td width="33%" align="center"><img src="../../.github/assets/tasks.webp" alt="Tasks：今天、明天和即将到来的任务，其中一个任务关联着它来自的页面" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/time.webp" alt="Time：今天上网 3 小时 24 分钟、一轮 25 分钟的专注和各网站用时" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/notes.webp" alt="Notes：页面旁的 Markdown 发布清单，含代码" /></td>
  </tr>
  <tr>
    <td valign="top"><strong>Tasks</strong><br /><sub>像说话一样写下来，比如“明天下午 3 点给 Anna 打电话”。支持列表、优先级、截止日期和子任务，你当时所在的页面也会保持关联。</sub></td>
    <td valign="top"><strong>Time</strong><br /><sub>看清你的上网时间花在了哪里，仅在本设备上统计。开始一轮 Focus，你选定的网站会保持关闭，直到休息时间。</sub></td>
    <td valign="top"><strong>Notes</strong><br /><sub>笔记就在页面旁边，每篇都是归你所有的 Markdown 文件。其他应用中的修改会同步显示在 Zephium 里。</sub></td>
  </tr>
</table>

## 默认保护隐私

- **没有遥测。** Zephium 不会收集或发送任何关于你如何浏览的信息。
- **无需账号。** 历史记录、任务、笔记、记忆和时间数据都保存在你的设备上。
- **隐私窗口关闭后不留任何内容。**

## 下载

| 平台 | 架构 | 安装包 | 系统要求 |
| -------- | ------------ | --------- | -------- |
| macOS | Apple Silicon | [`Zephium-macOS-arm64.dmg`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-macOS-arm64.dmg) | macOS Sonoma 14 或更高版本 |
| Windows | x64 | [`Zephium-Windows-x64-setup.exe`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-Windows-x64-setup.exe) | Windows 10 或 11 |

Windows 版本尚未进行代码签名，因此 SmartScreen 可能会显示“Windows 已保护你的电脑”。请选择 **更多信息**，然后点击 **仍要运行**。签名工作正在进行中。

Zephium 会在后台下载更新，并在你选择 **Relaunch to update** 时安装。所有版本都在[发布页面](https://github.com/zephium-browser/Zephium/releases)。

## 接下来

- Linux 和 Intel 版 Mac。
- 经过代码签名的 Windows 版本。
- 更多经过验证可用的扩展，以及内置于 Zephium 的原生扩展。
- 还有更多。

## 从源码构建

前置条件：

- 通过 `rustup` 安装 [Rust](https://rustup.rs)。[`rust-toolchain.toml`](../../rust-toolchain.toml) 中固定的工具链会在首次使用时自动安装。
- Node.js，版本见 [`.node-version`](../../.node-version)。
- pnpm，通过 Corepack 启用：`corepack enable`。
- 适用于你所在平台的 [Tauri 前置条件](https://v2.tauri.app/start/prerequisites/)：macOS 上是 Xcode Command Line Tools，Windows 上是 Microsoft C++ Build Tools 和 WebView2。

```sh
git clone https://github.com/zephium-browser/Zephium.git
cd Zephium
pnpm install --frozen-lockfile
pnpm dev
```

开发构建使用独立的 `app.zephium.dev` 配置文件，因此不会触碰已安装的 Zephium 的数据。目前尚不支持 Linux。提交 pull request 之前，请先阅读 [CONTRIBUTING.md](../../CONTRIBUTING.md)。

## 架构

- 不受信任的网页运行在原生 WebView 中，与绘制浏览器界面的特权 Svelte 界面相互隔离。
- Rust 核心负责标签页、存储、拦截器、扩展和智能体。
- 每个配置文件都有各自独立的网站数据，以及由 Zephium 管理的历史记录、会话和网站图标数据。
- 网络拦截器是原生的，基于 Brave 的 [adblock-rust](https://github.com/brave/adblock-rust) 构建。
- 精简的、位于仓库内的 Tauri 和 Wry 适配层负责存储策略、WebView 的创建、回调和销毁。

更多内容请阅读 [docs/architecture.md](../architecture.md) 和 [docs/security-model.md](../security-model.md)。

## 参与贡献

Zephium 由一个人维护，产品方向明确，因此较大的改动请先讨论再动手。[CONTRIBUTING.md](../../CONTRIBUTING.md) 说明了什么样的改动会被合并以及如何提交。欢迎在 [Discord](https://discord.gg/tyveTUyEp7) 上提问和分享想法。

## 安全

请私下报告漏洞，不要发在公开的 issue 里。参见 [SECURITY.md](../../SECURITY.md)。

## 许可证

Zephium 采用 [Mozilla Public License 2.0](../../LICENSE) 授权。

内置的 EasyList 和 EasyPrivacy 过滤规则依据 [CC BY-SA 3.0](../../assets/blocker-seed/v1/LICENSE-CC-BY-SA-3.0.txt) 使用，署名信息见[声明](../../assets/blocker-seed/v1/NOTICE)。

Zephium 的名称和标志不在 MPL 授权范围内。

## 致谢

Zephium 建立在 [Tauri](https://tauri.app)、[Wry](https://github.com/tauri-apps/wry)、Brave 的 [adblock-rust](https://github.com/brave/adblock-rust) 和 [EasyList](https://easylist.to) 之上。
