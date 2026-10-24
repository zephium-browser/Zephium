<div align="center">
  <img src="../../.github/assets/logo.png" width="112" height="112" alt="Zephium" />
  <h1>Zephium</h1>

  <p><strong>빠르고 기능이 충실한 브라우저이자 작업 환경,<br />당신과 에이전트를 위해 새롭게 다시 구상했습니다.</strong></p>

  <p>
    <a href="https://github.com/zephium-browser/Zephium/releases"><img src="https://img.shields.io/github/v/release/zephium-browser/Zephium?include_prereleases&label=release&color=blue" alt="최신 릴리스" /></a>
    <a href="../../LICENSE"><img src="https://img.shields.io/badge/license-MPL--2.0-blue" alt="라이선스: MPL-2.0" /></a>
    <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey" alt="지원 플랫폼: macOS 및 Windows" />
    <a href="https://discord.gg/tyveTUyEp7"><img src="https://img.shields.io/badge/Discord-5865F2?logo=discord&logoColor=white" alt="Discord" /></a>
    <a href="https://www.youtube.com/@crynta"><img src="https://img.shields.io/badge/YouTube-FF0000?logo=youtube&logoColor=white" alt="YouTube" /></a>
    <a href="https://x.com/CryntaDev"><img src="https://img.shields.io/badge/x.com-000000?logo=x&logoColor=white" alt="X: @CryntaDev" /></a>
  </p>

  <p>
    <a href="https://zephium.app">웹사이트</a> ·
    <a href="https://zephium.app/docs">문서</a> ·
    <a href="https://youtu.be/QbSarcWspvw">런칭 필름</a> ·
    <a href="https://github.com/zephium-browser/Zephium/releases/latest">다운로드</a>
  </p>
</div>

<details align="center">
  <summary><sub>다른 언어로 읽기</sub></summary>
  <sub>
    <a href="../../README.md">English</a> ·
    <a href="README.zh-CN.md">简体中文</a> ·
    <a href="README.ja.md">日本語</a> ·
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
    <img src="../../.github/assets/browse-light.webp" alt="Zephium의 새 탭. 창이 하늘을 향해 열리고, 아래쪽에 이름, 시간과 날짜, 그날의 수치가 놓여 있다" width="960" />
  </picture>
</p>

<p align="center">
  <strong>Safari의 효율. Brave의 보호. Arc의 디자인.</strong><br />
  그리고 <strong>Work</strong>, 에이전트가 눈앞에서 실제 작업을 해내는 캔버스.
</p>

<p align="center"><a href="https://youtu.be/QbSarcWspvw">▶&nbsp; 런칭 필름 보기</a></p>

---

Zephium은 Rust로 만든 오픈 소스 브라우저로, 운영체제에 내장된 웹 엔진 위에서 동작합니다. macOS에서는 Safari가 쓰는 엔진인 WebKit을, Windows에서는 WebView2를 사용합니다. Chromium이나 Firefox의 포크가 아닙니다. 광고와 트래커를 네이티브로 차단하고, Chrome 확장 프로그램을 실행하며, 작업, 메모, 웹에서 보내는 시간을 클릭 한 번이면 열 수 있게 해 줍니다. 다운로드 크기는 약 30 MB입니다.

스위치 하나만 누르면 **Work**가 열립니다. 당신의 작업은 이미 브라우저, 즉 탭과 로그인과 방문 기록이 있는 곳에서 이루어지고 있습니다. Work는 다른 곳으로 옮겨 가라고 하는 대신 에이전트를 이곳으로 데려오고, 에이전트가 일하는 모습을 지켜볼 수 있게 합니다.

## Work

원하는 결과를 평소 쓰는 말로 설명해 보세요. 여행 계획, 업체 비교, 시스템 설계, 버그 수정 같은 것들입니다. 그다음은 Work가 캔버스 위에서 이어받고, 모든 단계를 눈으로 확인할 수 있습니다.

<p align="center">
  <img src="../../.github/assets/work.webp" alt="Zephium에서 Work가 한 번 실행되는 전 과정. 도우미들이 Uptime Kuma, Gatus, OpenStatus를 각 사이트에서 읽고, 로컬 도우미가 엔드포인트를 확인한 뒤, 답변, 비교 표와 차트, 다이어그램, 설정, 작업 네 개가 이어진다" width="960" />
</p>
<p align="center"><sub>상태 페이지를 만들어 달라고 요청하면, Work는 모니터링 도구 세 가지를 각 사이트에서 읽고, 당신의 엔드포인트를 당신의 컴퓨터에서 확인해 최종안을 만든 다음, 비교, 설정, 작업을 캔버스에 남깁니다.</sub></p>

- **이미 가진 것에서 출발합니다.** Work는 당신에 대해 알고 있는 것을 떠올리고, 도움이 될 때는 방문 기록을 검색하며, 작업에 맞는 스킬을 불러옵니다. 각 단계는 진행되는 대로 캔버스에 나타납니다.
- **도우미들이 병렬로, 공개적으로 일합니다.** 여행이라면 한 도우미는 집을 찾고, 다른 도우미는 비자를 확인하고, 또 다른 도우미는 항공편을 비교합니다. 도우미들은 직접 볼 수 있는 실제 페이지를 탐색하고, 어떤 출처든 캔버스 옆 창에서 열 수 있습니다.
- **결과는 캔버스에 남습니다.** 비교, 표, 차트, 다이어그램, 계획, 코드, 문서가 읽기 좋은 자리에 배치됩니다. 흘러가는 채팅 속에 묻히지 않습니다.
- **계획이 하루가 됩니다.** 한 번 클릭하면 계획의 각 단계가 해당 Work에 연결된 작업이 됩니다. 남겨 둘 만한 내용은 무엇이든 메모로 저장할 수 있습니다.
- **브라우저 밖까지 닿습니다.** Work는 허용한 폴더를 읽고 편집하고, 명령을 실행하며, 규모가 큰 코딩 작업은 Claude Code나 Codex에 넘깁니다. Linear, Notion, Sentry, Stripe, Figma 같은 MCP 서버와 `gh` 같은 명령줄 도구에도 연결됩니다.
- **결정권은 당신에게 있습니다.** 게시하거나 병합하거나 무언가를 바꾸는 작업은 당신의 승인을 기다립니다.
- **어떤 모델이든 사용할 수 있습니다.** Anthropic, OpenAI, Google Gemini, DeepSeek, OpenRouter는 직접 발급한 키로 사용하고, Ollama, LM Studio 또는 OpenAI 호환 서버를 통해 로컬 모델을 실행할 수도 있습니다. 키는 시스템 키체인에 보관됩니다. 또는 Zephium AI에 로그인해 키 없이 시작할 수도 있습니다. 단계마다 알맞은 모델을 골라 주고, 무료 크레딧으로 시작할 수 있습니다([요금](https://zephium.app/pricing)).

Work에는 여행 계획, 리서치, 비교하고 선택하기, 내 하루 계획, 버그 수정, 주간 현황 등 25개의 스킬이 포함되어 있습니다. 직접 스킬을 만들 수도 있습니다.

## Ask Zephium

사이드바의 어시스턴트로, `⌘ E`(Windows에서는 `Ctrl E`)로 엽니다. 지금 보고 있는 페이지를 읽고 그에 대해 답하며, 시간을 들일 만한 것을 짚어 주고, 당신이 고른 것을 새 탭으로 엽니다. 페이지에서 몇 가지 단계를 대신 해 달라고 하거나, 매일 들르는 사이트의 스타일을 바꿔 달라고 하면 그 자리에서 해냅니다. 일이 채팅으로는 감당하기 어려워지면, **Continue in Work**가 페이지까지 함께 캔버스로 옮겨 줍니다.

<p align="center">
  <img src="../../.github/assets/ask.webp" alt="Hacker News 옆의 Ask Zephium. 기사 세 개를 골라 새 탭으로 열고, 첫 페이지를 차분한 카드로 다시 짠다" width="960" />
</p>
<p align="center"><sub>읽을 만한 게 뭐냐고 물으면, 기사 세 개를 골라 열고 첫 페이지를 차분한 카드로 다시 구성합니다.</sub></p>

## Browse

### 빠르고 가볍게

- macOS에서는 네이티브 WebKit, Windows에서는 WebView2를 사용하므로 페이지는 시스템이 이미 최신으로 관리하는 엔진에서 실행됩니다.
- 활성 상태가 아닌 탭은 잠들었다가 돌아오면 깨어나므로, 탭을 많이 열어 두어도 메모리 사용량이 낮게 유지됩니다.
- 기본으로 켜져 있는 네이티브 광고 및 트래커 차단기는 Brave의 [adblock-rust](https://github.com/brave/adblock-rust)를 기반으로 하며 EasyList와 EasyPrivacy를 사용합니다. 요청은 페이지가 보내기 전에 차단되고, 페이지의 다른 요소는 클릭 한 번으로 숨길 수 있습니다.

### 오래 머물 수 있게 설계했습니다

- 하늘을 향해 열리는 새 탭. 테마에 따라 낮이나 밤의 모습이 되고, 아래쪽에 시간, 집중, 그날의 작업이 놓입니다.
- 조용한 사이드바의 세로 탭, 그리고 스페이스, 프로필, 고정 탭, 폴더, 분할 보기.
- macOS 26에서는 Liquid Glass, Windows 11에서는 Mica.
- `⌘ ⇧ Space`(Windows에서는 `Ctrl Shift Space`)로 여는 런처로 데스크톱 어디서든 탭, 방문 기록, 메모, 명령에 접근할 수 있습니다. 거기에 입력한 내용은 무엇이든 작업으로 만들 수 있습니다.
- 모든 키보드 단축키를 바꿀 수 있습니다.

### 확장 프로그램

Chrome 웹 스토어에서 곧바로 확장 프로그램을 설치할 수 있습니다. 인기 있는 확장 프로그램 20개는 Zephium에서 잘 동작하는지 검증을 마쳤고, 확장 프로그램 관리자에서 클릭 한 번으로 설치할 수 있습니다. 1Password, Bitwarden, Grammarly, DeepL, Dark Reader, Vimium, SponsorBlock, Raindrop.io, Notion Web Clipper, Refined GitHub 등이 포함됩니다.

### 모든 것을 그대로 가져오세요

시작 화면에서 Chrome, Safari, Arc, Zen, Firefox, Brave, Edge의 데이터를 가져올 수 있습니다.

## 내장 기능

<table>
  <tr>
    <td width="33%" align="center"><img src="../../.github/assets/tasks.webp" alt="Tasks: 오늘, 내일, 예정된 작업과 출처 페이지에 연결된 작업" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/time.webp" alt="Time: 오늘 웹에서 보낸 3시간 24분, 25분 집중 라운드, 사이트별 시간" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/notes.webp" alt="Notes: 페이지 옆의 코드가 있는 Markdown 릴리스 체크리스트" /></td>
  </tr>
  <tr>
    <td valign="top"><strong>Tasks</strong><br /><sub>말하듯이 적으세요. 예를 들어 "내일 3시에 Anna에게 전화". 목록, 우선순위, 마감일, 하위 작업을 지원하고, 보고 있던 페이지도 연결된 채로 남습니다.</sub></td>
    <td valign="top"><strong>Time</strong><br /><sub>웹에서 시간이 어디에 쓰이는지 확인하세요. 집계는 이 기기에서만 이루어집니다. Focus 라운드를 시작하면 선택한 사이트는 휴식 시간까지 닫혀 있습니다.</sub></td>
    <td valign="top"><strong>Notes</strong><br /><sub>페이지 옆에 두는 메모로, 하나하나가 당신이 소유하는 Markdown 파일입니다. 다른 앱에서 고친 내용도 Zephium에 반영됩니다.</sub></td>
  </tr>
</table>

## 기본적으로 프라이버시를 지킵니다

- **텔레메트리가 없습니다.** Zephium은 당신이 어떻게 브라우징하는지에 대한 어떤 정보도 수집하거나 전송하지 않습니다.
- **계정이 필요 없습니다.** 방문 기록, 작업, 메모, 메모리, 시간 데이터는 당신의 기기에 저장됩니다.
- **프라이빗 창은 닫으면 아무것도 남기지 않습니다.**

## 다운로드

| 플랫폼 | 아키텍처 | 설치 파일 | 요구 사항 |
| -------- | ------------ | --------- | -------- |
| macOS | Apple Silicon | [`Zephium-macOS-arm64.dmg`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-macOS-arm64.dmg) | macOS Sonoma 14 이상 |
| Windows | x64 | [`Zephium-Windows-x64-setup.exe`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-Windows-x64-setup.exe) | Windows 10 또는 11 |

Windows 빌드는 아직 코드 서명이 되어 있지 않아 SmartScreen에서 "Windows의 PC 보호"라는 메시지가 표시될 수 있습니다. **추가 정보**를 선택한 다음 **실행**을 누르세요. 서명은 준비 중입니다.

Zephium은 업데이트를 백그라운드에서 내려받고, **Relaunch to update**를 선택하면 설치합니다. 모든 릴리스는 [릴리스 페이지](https://github.com/zephium-browser/Zephium/releases)에서 볼 수 있습니다.

## 앞으로의 계획

- Linux와 Intel Mac 지원.
- 코드 서명된 Windows 빌드.
- 동작이 확인된 확장 프로그램 추가, 그리고 Zephium에 내장되는 네이티브 확장 프로그램.
- 그리고 더 많은 것들.

## 소스에서 빌드하기

사전 준비:

- `rustup`으로 설치하는 [Rust](https://rustup.rs). [`rust-toolchain.toml`](../../rust-toolchain.toml)에 고정된 툴체인은 처음 사용할 때 자동으로 설치됩니다.
- [`.node-version`](../../.node-version)에 적힌 버전의 Node.js.
- Corepack으로 사용하는 pnpm: `corepack enable`.
- 사용 중인 플랫폼에 맞는 [Tauri 사전 요구 사항](https://v2.tauri.app/start/prerequisites/): macOS에서는 Xcode Command Line Tools, Windows에서는 Microsoft C++ Build Tools와 WebView2.

```sh
git clone https://github.com/zephium-browser/Zephium.git
cd Zephium
pnpm install --frozen-lockfile
pnpm dev
```

개발 빌드는 별도의 `app.zephium.dev` 프로필을 사용하므로 설치된 Zephium의 데이터를 건드리지 않습니다. Linux는 아직 지원하지 않습니다. 풀 리퀘스트를 열기 전에 [CONTRIBUTING.md](../../CONTRIBUTING.md)를 읽어 주세요.

## 아키텍처

- 신뢰할 수 없는 페이지는 브라우저 화면을 그리는 권한 있는 Svelte 인터페이스와 분리된 채, 순수한 네이티브 WebView에서 실행됩니다.
- Rust 코어가 탭, 저장소, 차단기, 확장 프로그램, 에이전트를 관리합니다.
- 각 프로필은 격리된 자체 웹사이트 데이터와, Zephium이 관리하는 방문 기록, 세션, 파비콘 데이터를 가집니다.
- 네트워크 차단기는 네이티브이며 Brave의 [adblock-rust](https://github.com/brave/adblock-rust)를 기반으로 합니다.
- 저장소 안에 있는 좁은 범위의 Tauri 및 Wry 어댑터가 저장 정책, WebView 생성, 콜백, 해제를 처리합니다.

자세한 내용은 [docs/architecture.md](../architecture.md)와 [docs/security-model.md](../security-model.md)에서 확인하세요.

## 기여하기

Zephium은 분명한 제품 방향을 가진 한 사람이 관리합니다. 큰 변경은 작성하기 전에 먼저 논의해 주세요. 무엇이 어떻게 병합되는지는 [CONTRIBUTING.md](../../CONTRIBUTING.md)에 설명되어 있습니다. 질문과 아이디어는 [Discord](https://discord.gg/tyveTUyEp7)에서 환영합니다.

## 보안

취약점은 공개 이슈가 아니라 비공개로 신고해 주세요. [SECURITY.md](../../SECURITY.md)를 참고하세요.

## 라이선스

Zephium은 [Mozilla Public License 2.0](../../LICENSE)에 따라 라이선스가 부여됩니다.

함께 제공되는 EasyList와 EasyPrivacy 필터 목록은 [CC BY-SA 3.0](../../assets/blocker-seed/v1/LICENSE-CC-BY-SA-3.0.txt)에 따라 사용되며, 저작자 표시는 [고지문](../../assets/blocker-seed/v1/NOTICE)을 참고하세요.

Zephium의 이름과 로고는 MPL 라이선스 대상이 아닙니다.

## 감사의 말

Zephium은 [Tauri](https://tauri.app), [Wry](https://github.com/tauri-apps/wry), Brave의 [adblock-rust](https://github.com/brave/adblock-rust), 그리고 [EasyList](https://easylist.to) 위에 서 있습니다.
