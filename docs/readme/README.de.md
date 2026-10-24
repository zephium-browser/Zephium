<div align="center">
  <img src="../../.github/assets/logo.png" width="112" height="112" alt="Zephium" />
  <h1>Zephium</h1>

  <p><strong>Ein schneller, voll ausgestatteter Browser samt Arbeitsumgebung,<br />neu gedacht für dich und deine Agenten.</strong></p>

  <p>
    <a href="https://github.com/zephium-browser/Zephium/releases"><img src="https://img.shields.io/github/v/release/zephium-browser/Zephium?include_prereleases&label=release&color=blue" alt="Neueste Version" /></a>
    <a href="../../LICENSE"><img src="https://img.shields.io/badge/license-MPL--2.0-blue" alt="Lizenz: MPL-2.0" /></a>
    <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey" alt="Plattformen: macOS und Windows" />
    <a href="https://discord.gg/tyveTUyEp7"><img src="https://img.shields.io/badge/Discord-5865F2?logo=discord&logoColor=white" alt="Discord" /></a>
    <a href="https://www.youtube.com/@crynta"><img src="https://img.shields.io/badge/YouTube-FF0000?logo=youtube&logoColor=white" alt="YouTube" /></a>
    <a href="https://x.com/CryntaDev"><img src="https://img.shields.io/badge/x.com-000000?logo=x&logoColor=white" alt="X: @CryntaDev" /></a>
  </p>

  <p>
    <a href="https://zephium.app">Website</a> ·
    <a href="https://zephium.app/docs">Dokumentation</a> ·
    <a href="https://youtu.be/QbSarcWspvw">Launch-Film</a> ·
    <a href="https://github.com/zephium-browser/Zephium/releases/latest">Download</a>
  </p>
</div>

<details align="center">
  <summary><sub>In einer anderen Sprache lesen</sub></summary>
  <sub>
    <a href="../../README.md">English</a> ·
    <a href="README.zh-CN.md">简体中文</a> ·
    <a href="README.ja.md">日本語</a> ·
    <a href="README.ko.md">한국어</a> ·
    <a href="README.hi.md">हिन्दी</a> ·
    <a href="README.es.md">Español</a> ·
    <a href="README.pt-BR.md">Português</a> ·
    <a href="README.fr.md">Français</a> ·
    <a href="README.pl.md">Polski</a> ·
    <a href="README.ru.md">Русский</a> ·
    <a href="README.id.md">Bahasa Indonesia</a>
  </sub>
</details>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../../.github/assets/browse-dark.webp" />
    <img src="../../.github/assets/browse-light.webp" alt="Der neue Tab von Zephium: Das Fenster öffnet sich zum Himmel, am unteren Rand stehen der Name, Uhrzeit und Datum und die Zahlen des Tages" width="960" />
  </picture>
</p>

<p align="center">
  <strong>Safaris Effizienz. Braves Schutz. Arcs Design.</strong><br />
  Und <strong>Work</strong>, eine Arbeitsfläche, auf der deine Agenten echte Arbeit vor aller Augen erledigen.
</p>

<p align="center"><a href="https://youtu.be/QbSarcWspvw">▶&nbsp; Den Launch-Film ansehen</a></p>

---

Zephium ist ein Open-Source-Browser, in Rust gebaut auf der Web-Engine deines
Betriebssystems: WebKit unter macOS, der Engine von Safari, und WebView2 unter
Windows. Er ist kein Fork von Chromium oder Firefox. Er blockiert Werbung und
Tracker nativ, führt Chrome-Erweiterungen aus und legt Aufgaben, Notizen und
deine Zeit im Web nur einen Klick entfernt. Der Download ist etwa 30 MB groß.

Nur einen Schalter entfernt liegt **Work**. Deine Arbeit findet ohnehin im
Browser statt, dort, wo deine Tabs, Logins und dein Verlauf sind. Work bringt
Agenten dorthin, statt dich an einen anderen Ort umziehen zu lassen, und du
kannst ihnen dabei zusehen.

## Work

Beschreibe ein Ergebnis in deinen eigenen Worten: eine Reise planen, Anbieter
vergleichen, ein System entwerfen, einen Bug beheben. Work übernimmt von dort
auf einer Arbeitsfläche, und du siehst jeden Schritt.

<p align="center">
  <img src="../../.github/assets/work.webp" alt="Ein ganzer Work-Durchlauf in Zephium: Helfer lesen Uptime Kuma, Gatus und OpenStatus auf deren eigenen Seiten, ein lokaler Helfer prüft die Endpunkte, dann folgen die Antwort, eine Vergleichstabelle samt Diagrammen, ein Schaubild, die Konfiguration und vier Aufgaben" width="960" />
</p>
<p align="center"><sub>Auf die Bitte um eine Statusseite liest Work drei Monitoring-Tools auf deren eigenen Seiten, prüft deine Endpunkte von deinem Rechner aus, baut die Empfehlung zusammen und lässt Vergleich, Konfiguration und Aufgaben auf der Arbeitsfläche liegen.</sub></p>

- **Es beginnt bei dem, was du schon hast.** Work ruft ab, was es über dich
  weiß, durchsucht bei Bedarf deinen Verlauf und lädt den passenden Skill für
  die Aufgabe. Jeder Schritt erscheint auf der Arbeitsfläche, während er
  passiert.
- **Helfer arbeiten parallel und offen.** Bei einer Reise sucht ein Helfer
  Wohnungen, während ein anderer Visa prüft und ein dritter Flüge vergleicht.
  Sie surfen auf echten Seiten, die du beobachten kannst, und jede Quelle öffnet
  sich in einem Bereich neben der Arbeitsfläche.
- **Ergebnisse bleiben auf der Arbeitsfläche.** Vergleiche, Tabellen,
  Diagramme, Schaubilder, Pläne, Code und Dokumente sind dort angeordnet, wo du sie
  lesen kannst. Sie gehen nicht im Scrollen eines Chats verloren.
- **Aus einem Plan wird dein Tag.** Ein Klick macht aus jedem Schritt eines Plans
  eine Aufgabe, die mit dem jeweiligen Work verknüpft ist. Alles, was sich
  aufzuheben lohnt, lässt sich als Notiz speichern.
- **Es reicht über den Browser hinaus.** Work liest und bearbeitet die Ordner,
  die du freigibst, führt Befehle aus und übergibt größere Programmieraufgaben
  an Claude Code oder Codex. Es verbindet sich mit MCP-Servern wie Linear,
  Notion, Sentry, Stripe und Figma sowie mit Kommandozeilen-Tools wie `gh`.
- **Du behältst die Kontrolle.** Alles, was etwas veröffentlicht, zusammenführt
  oder verändert, wartet auf dein OK.
- **Jedes Modell ist möglich.** Nutze deine eigenen Schlüssel für Anthropic,
  OpenAI, Google Gemini, DeepSeek oder OpenRouter, oder betreibe ein Modell
  lokal über Ollama, LM Studio oder einen beliebigen OpenAI-kompatiblen Server.
  Die Schlüssel liegen in der Schlüsselverwaltung deines Systems.
  Oder melde dich bei Zephium AI an und verzichte auf die Schlüssel: Es wählt für
  jeden Schritt das passende Modell, und du startest mit Gratis-Guthaben
  ([Preise](https://zephium.app/pricing)).

Work wird mit 25 Skills ausgeliefert, darunter Reiseplanung, Recherche,
Vergleichen und Entscheiden, meinen Tag planen, einen Bug beheben und
Wochenstatus. Du kannst eigene schreiben.

## Ask Zephium

Ein Assistent in der Seitenleiste, aufrufbar mit `⌘ E` (`Ctrl E` unter Windows).
Er liest die Seite, auf der du gerade bist, und beantwortet Fragen dazu, zeigt,
was deine Zeit wert ist, und öffnet das, was du auswählst, in neuen Tabs. Bitte
ihn, ein paar Schritte auf der Seite auszuführen oder eine Website, die du jeden
Tag besuchst, neu zu gestalten, und er erledigt es an Ort und Stelle. Wächst eine
Aufgabe über einen Chat hinaus, nimmt **Continue in Work** sie samt Seite mit auf
die Arbeitsfläche.

<p align="center">
  <img src="../../.github/assets/ask.webp" alt="Ask Zephium neben Hacker News: drei Beiträge ausgewählt, in neuen Tabs geöffnet und die Startseite als ruhige Karten neu gesetzt" width="960" />
</p>
<p align="center"><sub>Auf die Frage, was sich zu lesen lohnt, wählt er drei Beiträge aus, öffnet sie und setzt die Startseite als ruhige Karten neu.</sub></p>

## Browse

### Schnell und leicht

- Natives WebKit unter macOS und WebView2 unter Windows, damit Seiten auf der
  Engine laufen, die dein System ohnehin aktuell hält.
- Inaktive Tabs gehen in den Ruhezustand und wachen auf, sobald du zu ihnen
  zurückkehrst. So bleibt der Speicherverbrauch auch mit vielen offenen Tabs
  niedrig.
- Ein nativer Werbe- und Trackerblocker, standardmäßig aktiv, aufgebaut auf
  Braves [adblock-rust](https://github.com/brave/adblock-rust) mit EasyList und
  EasyPrivacy. Anfragen werden gestoppt, bevor eine Seite sie stellen kann, und
  alles andere auf einer Seite kannst du mit einem Klick ausblenden.

### Gemacht, um darin zu leben

- Ein neuer Tab, der sich zum Himmel öffnet, je nach Theme bei Tag oder bei Nacht,
  mit deiner Uhrzeit, deinem Fokus und den Aufgaben des Tages am unteren Rand.
- Vertikale Tabs in einer ruhigen Seitenleiste, mit Spaces, Profilen, angehefteten
  Tabs, Ordnern und geteilter Ansicht.
- Liquid Glass unter macOS 26 und Mica unter Windows 11.
- Ein Launcher auf `⌘ ⇧ Space` (`Ctrl Shift Space` unter Windows) erreicht Tabs,
  Verlauf, Notizen und Befehle von überall auf deinem Desktop. Alles, was du dort
  eintippst, kann zu einer Aufgabe werden.
- Jedes Tastaturkürzel lässt sich ändern.

### Erweiterungen

Installiere Erweiterungen direkt aus dem Chrome Web Store. Zwanzig beliebte sind
darauf geprüft, in Zephium gut zu funktionieren, und liegen in der
Erweiterungsverwaltung einen Klick entfernt, darunter 1Password, Bitwarden,
Grammarly, DeepL, Dark Reader, Vimium, SponsorBlock, Raindrop.io, Notion Web
Clipper und Refined GitHub.

### Nimm alles mit

Der Willkommensassistent importiert aus Chrome, Safari, Arc, Zen, Firefox, Brave
und Edge.

## Eingebaut

<table>
  <tr>
    <td width="33%" align="center"><img src="../../.github/assets/tasks.webp" alt="Tasks: heute, morgen und demnächst, mit einer Aufgabe, die mit der Seite verknüpft ist, von der sie stammt" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/time.webp" alt="Time: heute 3 Stunden 24 Minuten im Web, eine Fokusrunde von 25 Minuten und die Zeit pro Website" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/notes.webp" alt="Notes: eine Release-Checkliste in Markdown mit Code, neben der Seite" /></td>
  </tr>
  <tr>
    <td valign="top"><strong>Tasks</strong><br /><sub>Schreib sie so, wie du sie sagen würdest, etwa "Anna morgen um 15 Uhr anrufen". Listen, Prioritäten, Fristen, Unteraufgaben, und die Seite, auf der du warst, bleibt verknüpft.</sub></td>
    <td valign="top"><strong>Time</strong><br /><sub>Sieh, wohin deine Zeit im Web geht, gezählt nur auf diesem Gerät. Starte eine Focus-Runde, und die Websites, die du wählst, bleiben bis zur Pause gesperrt.</sub></td>
    <td valign="top"><strong>Notes</strong><br /><sub>Notizen neben der Seite, jede eine Markdown-Datei, die dir gehört. Änderungen aus anderen Apps erscheinen in Zephium.</sub></td>
  </tr>
</table>

## Standardmäßig privat

- **Keine Telemetrie.** Zephium erfasst und sendet nichts darüber, wie du surfst.
- **Kein Konto nötig.** Verlauf, Aufgaben, Notizen, Erinnerungen und Zeit bleiben auf
  deinem Gerät.
- **Private Fenster behalten nichts**, sobald sie geschlossen werden.

## Download

| Plattform | Architektur | Installer | Voraussetzung |
| --------- | ----------- | --------- | ------------- |
| macOS | Apple Silicon | [`Zephium-macOS-arm64.dmg`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-macOS-arm64.dmg) | macOS Sonoma 14 oder neuer |
| Windows | x64 | [`Zephium-Windows-x64-setup.exe`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-Windows-x64-setup.exe) | Windows 10 oder 11 |

Windows-Builds sind noch nicht code-signiert, daher zeigt SmartScreen
möglicherweise "Der Computer wurde durch Windows geschützt". Wähle **Weitere
Informationen** und dann **Trotzdem ausführen**. Die Signierung ist in Arbeit.

Zephium lädt Updates im Hintergrund herunter und installiert sie, sobald du
**Relaunch to update** wählst. Alle Releases findest du auf der
[Releases-Seite](https://github.com/zephium-browser/Zephium/releases).

## Wie es weitergeht

- Linux und Intel-Macs.
- Code-signierte Windows-Builds.
- Mehr Erweiterungen, die nachweislich funktionieren, und native Erweiterungen direkt in Zephium.
- Und vieles mehr.

## Aus dem Quellcode bauen

Voraussetzungen:

- [Rust](https://rustup.rs) über `rustup`. Die in
  [`rust-toolchain.toml`](../../rust-toolchain.toml) festgelegte Toolchain
  installiert sich bei der ersten Verwendung selbst.
- Node.js, in der Version aus [`.node-version`](../../.node-version).
- pnpm, über Corepack: `corepack enable`.
- Die [Tauri-Voraussetzungen](https://v2.tauri.app/start/prerequisites/) für
  deine Plattform: Xcode Command Line Tools unter macOS bzw. die Microsoft C++
  Build Tools und WebView2 unter Windows.

```sh
git clone https://github.com/zephium-browser/Zephium.git
cd Zephium
pnpm install --frozen-lockfile
pnpm dev
```

Entwicklungs-Builds verwenden ein eigenes Profil `app.zephium.dev` und berühren
deshalb nicht die Daten eines installierten Zephium. Linux wird noch nicht
unterstützt. Lies [CONTRIBUTING.md](../../CONTRIBUTING.md), bevor du einen Pull
Request öffnest.

## Architektur

- Nicht vertrauenswürdige Seiten laufen in rohen nativen WebViews, getrennt von
  der privilegierten Svelte-Oberfläche, die den Browser zeichnet.
- Ein Rust-Kern verwaltet Tabs, Speicher, den Blocker, Erweiterungen und Agenten.
- Jedes Profil hat eigene, isolierte Website-Daten sowie von Zephium verwaltete
  Daten für Verlauf, Sitzung und Favicons.
- Der Netzwerk-Blocker ist nativ und auf Braves
  [adblock-rust](https://github.com/brave/adblock-rust) aufgebaut.
- Schmale, im Repository liegende Adapter für Tauri und Wry kümmern sich um
  Speicherrichtlinien, den Aufbau von WebViews, Callbacks und den Abbau.

Mehr dazu in [docs/architecture.md](../architecture.md) und
[docs/security-model.md](../security-model.md).

## Mitwirken

Zephium wird von einer einzelnen Person mit klarer Produktrichtung gepflegt.
Bitte besprich größere Änderungen, bevor du sie schreibst.
[CONTRIBUTING.md](../../CONTRIBUTING.md) erklärt, was übernommen wird und wie.
Fragen und Ideen sind auf [Discord](https://discord.gg/tyveTUyEp7) willkommen.

## Sicherheit

Bitte melde Sicherheitslücken privat und nicht in einem öffentlichen Issue. Siehe
[SECURITY.md](../../SECURITY.md).

## Lizenz

Zephium steht unter der [Mozilla Public License 2.0](../../LICENSE).

Die mitgelieferten Filterlisten EasyList und EasyPrivacy werden unter
[CC BY-SA 3.0](../../assets/blocker-seed/v1/LICENSE-CC-BY-SA-3.0.txt) verwendet;
die Angaben zur Namensnennung stehen in
[diesem Hinweis](../../assets/blocker-seed/v1/NOTICE).

Name und Logo von Zephium sind nicht unter der MPL lizenziert.

## Danksagung

Zephium baut auf [Tauri](https://tauri.app), [Wry](https://github.com/tauri-apps/wry),
Braves [adblock-rust](https://github.com/brave/adblock-rust) und
[EasyList](https://easylist.to).
