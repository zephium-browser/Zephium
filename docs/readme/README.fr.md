<div align="center">
  <img src="../../.github/assets/logo.png" width="112" height="112" alt="Zephium" />
  <h1>Zephium</h1>

  <p><strong>Un navigateur et un environnement de travail rapides et complets,<br />réinventés pour vous et vos agents.</strong></p>

  <p>
    <a href="https://github.com/zephium-browser/Zephium/releases"><img src="https://img.shields.io/github/v/release/zephium-browser/Zephium?include_prereleases&label=release&color=blue" alt="Dernière version" /></a>
    <a href="../../LICENSE"><img src="https://img.shields.io/badge/license-MPL--2.0-blue" alt="Licence : MPL-2.0" /></a>
    <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey" alt="Plateformes : macOS et Windows" />
    <a href="https://discord.gg/tyveTUyEp7"><img src="https://img.shields.io/badge/Discord-5865F2?logo=discord&logoColor=white" alt="Discord" /></a>
    <a href="https://www.youtube.com/@crynta"><img src="https://img.shields.io/badge/YouTube-FF0000?logo=youtube&logoColor=white" alt="YouTube" /></a>
    <a href="https://x.com/CryntaDev"><img src="https://img.shields.io/badge/x.com-000000?logo=x&logoColor=white" alt="X: @CryntaDev" /></a>
  </p>

  <p>
    <a href="https://zephium.app">Site web</a> ·
    <a href="https://zephium.app/docs">Documentation</a> ·
    <a href="https://youtu.be/QbSarcWspvw">Film de lancement</a> ·
    <a href="https://github.com/zephium-browser/Zephium/releases/latest">Télécharger</a>
  </p>
</div>

<details align="center">
  <summary><sub>Lire dans une autre langue</sub></summary>
  <sub>
    <a href="../../README.md">English</a> ·
    <a href="README.zh-CN.md">简体中文</a> ·
    <a href="README.ja.md">日本語</a> ·
    <a href="README.ko.md">한국어</a> ·
    <a href="README.hi.md">हिन्दी</a> ·
    <a href="README.es.md">Español</a> ·
    <a href="README.pt-BR.md">Português</a> ·
    <a href="README.de.md">Deutsch</a> ·
    <a href="README.pl.md">Polski</a> ·
    <a href="README.ru.md">Русский</a> ·
    <a href="README.id.md">Bahasa Indonesia</a>
  </sub>
</details>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../../.github/assets/browse-dark.webp" />
    <img src="../../.github/assets/browse-light.webp" alt="Le nouvel onglet de Zephium : la fenêtre s'ouvre sur le ciel, avec le nom, l'heure et la date, et les chiffres du jour en bas" width="960" />
  </picture>
</p>

<p align="center">
  <strong>L'efficacité de Safari. La protection de Brave. Le design d'Arc.</strong><br />
  Et <strong>Work</strong>, un canevas où vos agents font un vrai travail, sous vos yeux.
</p>

<p align="center"><a href="https://youtu.be/QbSarcWspvw">▶&nbsp; Voir le film de lancement</a></p>

---

Zephium est un navigateur open source développé en Rust sur le moteur web propre
à votre système d'exploitation : WebKit sur macOS, le moteur de Safari, et
WebView2 sur Windows. Ce n'est pas un fork de Chromium ni de Firefox. Il bloque
nativement les publicités et les traqueurs, exécute les extensions Chrome et met
à un clic les tâches, les notes et votre temps passé sur le web. Le
téléchargement pèse environ 30 Mo.

**Work** n'est qu'à un interrupteur de distance. Votre travail se passe déjà
dans le navigateur, là où se trouvent vos onglets, vos connexions et votre
historique. Work y amène les agents au lieu de vous demander d'aller ailleurs,
et vous laisse les regarder à l'œuvre.

## Work

Décrivez un résultat avec vos propres mots : organiser un voyage, comparer des
prestataires, concevoir un système, corriger un bug. Work prend le relais sur un
canevas, et vous voyez chaque étape.

<p align="center">
  <img src="../../.github/assets/work.webp" alt="Une session Work complète dans Zephium : des assistants qui lisent Uptime Kuma, Gatus et OpenStatus sur leurs propres sites, un assistant local qui vérifie les endpoints, puis la réponse, un tableau comparatif et des graphiques, un schéma, la configuration et quatre tâches" width="960" />
</p>
<p align="center"><sub>Chargé de produire une page de statut, Work lit trois outils de supervision sur leurs propres sites, vérifie vos endpoints depuis votre machine, assemble le choix retenu et laisse sur le canevas le comparatif, la configuration et les tâches.</sub></p>

- **Il part de ce que vous avez déjà.** Work se rappelle ce qu'il sait de vous,
  fouille votre historique quand c'est utile et charge la bonne skill pour la
  tâche. Chaque étape apparaît sur le canevas au moment où elle a lieu.
- **Les assistants travaillent en parallèle, à découvert.** Pour un voyage, un
  assistant cherche des logements pendant qu'un autre vérifie les visas et
  qu'un troisième compare les vols. Ils naviguent sur de vraies pages que vous
  pouvez suivre, et toute source s'ouvre dans un volet à côté du canevas.
- **Les résultats restent sur le canevas.** Comparatifs, tableaux, graphiques,
  schémas, plans, code et documents sont disposés là où vous pouvez les lire. Ils
  ne se perdent pas dans le défilement d'un chat.
- **Un plan devient votre journée.** Un clic transforme chaque étape d'un plan
  en tâche liée au Work d'origine. Tout ce qui mérite d'être gardé peut être
  enregistré en note.
- **Il va au-delà du navigateur.** Work lit et modifie les dossiers que vous
  autorisez, exécute des commandes et confie les gros chantiers de code à Claude
  Code ou à Codex. Il se connecte à des serveurs MCP comme Linear, Notion,
  Sentry, Stripe et Figma, ainsi qu'à des outils en ligne de commande comme
  `gh`.
- **Vous gardez la main.** Tout ce qui publie, fusionne ou modifie quelque chose
  attend votre accord.
- **Le modèle de votre choix.** Utilisez vos propres clés Anthropic, OpenAI,
  Google Gemini, DeepSeek ou OpenRouter, ou faites tourner un modèle en local
  avec Ollama, LM Studio ou n'importe quel serveur compatible OpenAI. Les clés
  sont conservées dans le trousseau de votre système.
  Ou connectez-vous à Zephium AI et oubliez les clés : il choisit le bon modèle
  à chaque étape, et vous démarrez avec des crédits offerts
  ([tarifs](https://zephium.app/pricing)).

Work est livré avec 25 skills, dont l'organisation de voyage, la recherche,
comparer et choisir, planifier ma journée, corriger un bug et le point
hebdomadaire. Vous pouvez écrire les vôtres.

## Ask Zephium

Un assistant dans la barre latérale, sur `⌘ E` (`Ctrl E` sur Windows). Il lit la
page où vous êtes et répond à son sujet, signale ce qui mérite votre temps et
ouvre dans de nouveaux onglets ce que vous choisissez. Demandez-lui de faire
quelques gestes sur la page, ou de restyler un site que vous visitez chaque
jour, et il le fait sur place. Quand une tâche dépasse le cadre d'une
conversation, **Continue in Work** l'emmène sur le canevas, avec la page.

<p align="center">
  <img src="../../.github/assets/ask.webp" alt="Ask Zephium à côté de Hacker News : trois articles choisis, ouverts dans de nouveaux onglets, et la page d'accueil remise en forme en cartes apaisées" width="960" />
</p>
<p align="center"><sub>Invité à dire ce qui vaut la peine d'être lu, il choisit trois articles, les ouvre et remet la page d'accueil en forme, en cartes apaisées.</sub></p>

## Browse

### Rapide et léger

- WebKit natif sur macOS et WebView2 sur Windows : les pages tournent sur le
  moteur que votre système tient déjà à jour.
- Les onglets inactifs se mettent en veille et se réveillent à votre retour, si
  bien que la mémoire reste basse même avec de nombreux onglets ouverts.
- Un bloqueur natif de publicités et de traqueurs, activé par défaut, bâti sur
  [adblock-rust](https://github.com/brave/adblock-rust) de Brave avec EasyList
  et EasyPrivacy. Les requêtes sont stoppées avant qu'une page puisse les
  émettre, et vous pouvez masquer d'un clic tout autre élément d'une page.

### Pensé pour y vivre

- Un nouvel onglet qui s'ouvre sur le ciel, de jour ou de nuit selon votre thème,
  avec l'heure, votre concentration et les tâches du jour en bas.
- Onglets verticaux dans une barre latérale discrète, avec espaces, profils,
  onglets épinglés, dossiers et vue scindée.
- Liquid Glass sur macOS 26 et Mica sur Windows 11.
- Un lanceur sur `⌘ ⇧ Space` (`Ctrl Shift Space` sous Windows) donne accès aux
  onglets, à l'historique, aux notes et aux commandes depuis n'importe où sur
  votre bureau. Tout ce que vous y saisissez peut devenir une tâche.
- Chaque raccourci clavier peut être modifié.

### Extensions

Installez des extensions directement depuis le Chrome Web Store. Vingt des plus
populaires ont été vérifiées pour bien fonctionner dans Zephium et sont à un
clic dans le gestionnaire d'extensions, dont 1Password, Bitwarden, Grammarly,
DeepL, Dark Reader, Vimium, SponsorBlock, Raindrop.io, Notion Web Clipper et
Refined GitHub.

### Emportez tout avec vous

Le parcours d'accueil importe vos données depuis Chrome, Safari, Arc, Zen,
Firefox, Brave et Edge.

## Intégré

<table>
  <tr>
    <td width="33%" align="center"><img src="../../.github/assets/tasks.webp" alt="Tasks : aujourd'hui, demain et à venir, avec une tâche liée à la page d'où elle vient" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/time.webp" alt="Time : 3 h 24 min sur le web aujourd'hui, une session de concentration de 25 minutes et le temps par site" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/notes.webp" alt="Notes : une checklist de version en Markdown avec du code, à côté de la page" /></td>
  </tr>
  <tr>
    <td valign="top"><strong>Tasks</strong><br /><sub>Écrivez-les comme vous les diriez, par exemple « appeler Anna demain à 15 h ». Listes, priorités, échéances, sous-tâches, et la page où vous étiez reste liée.</sub></td>
    <td valign="top"><strong>Time</strong><br /><sub>Voyez où passe votre temps sur le web, compté uniquement sur cet appareil. Lancez une session de Focus et les sites que vous avez choisis restent fermés jusqu'à la pause.</sub></td>
    <td valign="top"><strong>Notes</strong><br /><sub>Des notes à côté de la page, chacune un fichier Markdown qui vous appartient. Les modifications faites depuis d'autres apps apparaissent dans Zephium.</sub></td>
  </tr>
</table>

## Privé par défaut

- **Aucune télémétrie.** Zephium ne collecte ni n'envoie rien sur la façon dont vous naviguez.
- **Aucun compte requis.** L'historique, les tâches, les notes, la mémoire et le
  temps restent sur votre appareil.
- **Les fenêtres privées ne gardent rien** une fois fermées.

## Téléchargement

| Plateforme | Architecture | Installeur | Prérequis |
| ---------- | ------------ | ---------- | --------- |
| macOS | Apple Silicon | [`Zephium-macOS-arm64.dmg`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-macOS-arm64.dmg) | macOS Sonoma 14 ou version ultérieure |
| Windows | x64 | [`Zephium-Windows-x64-setup.exe`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-Windows-x64-setup.exe) | Windows 10 ou 11 |

Les versions Windows ne sont pas encore signées, SmartScreen peut donc afficher
« Windows a protégé votre ordinateur ». Choisissez **Informations
complémentaires**, puis **Exécuter quand même**. La signature est en cours.

Zephium télécharge les mises à jour en arrière-plan et les installe lorsque vous
choisissez **Relaunch to update**. Toutes les versions sont sur la
[page des releases](https://github.com/zephium-browser/Zephium/releases).

## Et ensuite

- Linux et les Mac Intel.
- Des versions Windows signées.
- Davantage d'extensions dont le fonctionnement est vérifié, et des extensions natives intégrées à Zephium.
- Et bien plus encore.

## Compiler depuis les sources

Prérequis :

- [Rust](https://rustup.rs) via `rustup`. La toolchain épinglée dans
  [`rust-toolchain.toml`](../../rust-toolchain.toml) s'installe d'elle-même au
  premier usage.
- Node.js, dans la version indiquée par [`.node-version`](../../.node-version).
- pnpm, via Corepack : `corepack enable`.
- Les [prérequis de Tauri](https://v2.tauri.app/start/prerequisites/) pour votre
  plateforme : Xcode Command Line Tools sur macOS, ou Microsoft C++ Build Tools
  et WebView2 sur Windows.

```sh
git clone https://github.com/zephium-browser/Zephium.git
cd Zephium
pnpm install --frozen-lockfile
pnpm dev
```

Les builds de développement utilisent leur propre profil `app.zephium.dev` et ne
touchent donc pas aux données d'un Zephium installé. Linux n'est pas encore pris
en charge. Lisez [CONTRIBUTING.md](../../CONTRIBUTING.md) avant d'ouvrir une
pull request.

## Architecture

- Les pages non fiables s'exécutent dans des WebViews natives brutes, séparées
  de l'interface Svelte privilégiée qui dessine le navigateur.
- Un cœur en Rust gère les onglets, le stockage, le bloqueur, les extensions et
  les agents.
- Chaque profil a ses propres données de sites web isolées, ainsi qu'un
  historique, une session et des favicons gérés par Zephium.
- Le bloqueur réseau est natif et bâti sur
  [adblock-rust](https://github.com/brave/adblock-rust) de Brave.
- De petits adaptateurs internes pour Tauri et Wry gèrent la politique de
  stockage, la construction des WebViews, les callbacks et la fermeture.

Pour en savoir plus, lisez [docs/architecture.md](../architecture.md) et
[docs/security-model.md](../security-model.md).

## Contribuer

Zephium est maintenu par une seule personne qui a une direction produit claire :
merci de discuter des changements importants avant de les écrire.
[CONTRIBUTING.md](../../CONTRIBUTING.md) explique ce qui est intégré et
comment. Questions et idées sont les bienvenues sur
[Discord](https://discord.gg/tyveTUyEp7).

## Sécurité

Merci de signaler les vulnérabilités en privé, pas dans une issue publique.
Voir [SECURITY.md](../../SECURITY.md).

## Licence

Zephium est distribué sous la [Mozilla Public License 2.0](../../LICENSE).

Les listes de filtres EasyList et EasyPrivacy incluses sont utilisées sous
[CC BY-SA 3.0](../../assets/blocker-seed/v1/LICENSE-CC-BY-SA-3.0.txt) ; voir
[l'avis](../../assets/blocker-seed/v1/NOTICE) pour l'attribution.

Le nom et le logo de Zephium ne sont pas placés sous licence MPL.

## Remerciements

Zephium s'appuie sur [Tauri](https://tauri.app), [Wry](https://github.com/tauri-apps/wry),
sur [adblock-rust](https://github.com/brave/adblock-rust) de Brave et sur
[EasyList](https://easylist.to).
