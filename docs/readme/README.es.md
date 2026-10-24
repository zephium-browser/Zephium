<div align="center">
  <img src="../../.github/assets/logo.png" width="112" height="112" alt="Zephium" />
  <h1>Zephium</h1>

  <p><strong>Un navegador y un entorno de trabajo rápidos y completos,<br />reimaginados para ti y tus agentes.</strong></p>

  <p>
    <a href="https://github.com/zephium-browser/Zephium/releases"><img src="https://img.shields.io/github/v/release/zephium-browser/Zephium?include_prereleases&label=release&color=blue" alt="Última versión" /></a>
    <a href="../../LICENSE"><img src="https://img.shields.io/badge/license-MPL--2.0-blue" alt="Licencia: MPL-2.0" /></a>
    <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey" alt="Plataformas: macOS y Windows" />
    <a href="https://discord.gg/tyveTUyEp7"><img src="https://img.shields.io/badge/Discord-5865F2?logo=discord&logoColor=white" alt="Discord" /></a>
    <a href="https://www.youtube.com/@crynta"><img src="https://img.shields.io/badge/YouTube-FF0000?logo=youtube&logoColor=white" alt="YouTube" /></a>
    <a href="https://x.com/CryntaDev"><img src="https://img.shields.io/badge/x.com-000000?logo=x&logoColor=white" alt="X: @CryntaDev" /></a>
  </p>

  <p>
    <a href="https://zephium.app">Sitio web</a> ·
    <a href="https://zephium.app/docs">Documentación</a> ·
    <a href="https://youtu.be/QbSarcWspvw">Película de lanzamiento</a> ·
    <a href="https://github.com/zephium-browser/Zephium/releases/latest">Descargar</a>
  </p>
</div>

<details align="center">
  <summary><sub>Leer en otro idioma</sub></summary>
  <sub>
    <a href="../../README.md">English</a> ·
    <a href="README.zh-CN.md">简体中文</a> ·
    <a href="README.ja.md">日本語</a> ·
    <a href="README.ko.md">한국어</a> ·
    <a href="README.hi.md">हिन्दी</a> ·
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
    <img src="../../.github/assets/browse-light.webp" alt="La pestaña nueva de Zephium: la ventana se abre al cielo, con el nombre, la hora y la fecha, y las cifras del día en la parte inferior" width="960" />
  </picture>
</p>

<p align="center">
  <strong>La eficiencia de Safari. La protección de Brave. El diseño de Arc.</strong><br />
  Y <strong>Work</strong>, un lienzo donde tus agentes hacen trabajo real a la vista de todos.
</p>

<p align="center"><a href="https://youtu.be/QbSarcWspvw">▶&nbsp; Ver la película de lanzamiento</a></p>

---

Zephium es un navegador de código abierto, creado en Rust sobre el motor web
propio de tu sistema operativo: WebKit en macOS, el motor que usa Safari, y
WebView2 en Windows. No es un fork de Chromium ni de Firefox. Bloquea anuncios y
rastreadores de forma nativa, ejecuta extensiones de Chrome y deja a un clic las
tareas, las notas y tu tiempo en la web. La descarga pesa unos 30 MB.

A un solo interruptor está **Work**. Tu trabajo ya ocurre en el navegador, donde
están tus pestañas, tus sesiones iniciadas y tu historial. Work lleva a los
agentes hasta allí en lugar de pedirte que te mudes a otro sitio, y te deja
verlos trabajar.

## Work

Describe un resultado con tus propias palabras: planificar un viaje, comparar
proveedores, diseñar un sistema, corregir un error. Work se encarga a partir de
ahí, en un lienzo, y tú ves cada paso.

<p align="center">
  <img src="../../.github/assets/work.webp" alt="Una ejecución completa de Work en Zephium: ayudantes que leen Uptime Kuma, Gatus y OpenStatus en sus propios sitios, un ayudante local que comprueba los endpoints y, después, la respuesta, una tabla comparativa con gráficos, un diagrama, la configuración y cuatro tareas" width="960" />
</p>
<p align="center"><sub>Al pedirle una página de estado, Work lee tres monitores en sus propios sitios, comprueba tus endpoints desde tu equipo, monta la opción elegida y deja en el lienzo la comparación, la configuración y las tareas.</sub></p>

- **Parte de lo que ya tienes.** Work recuerda lo que sabe de ti, busca en tu
  historial cuando ayuda y carga la skill adecuada para cada tarea. Cada paso
  aparece en el lienzo a medida que ocurre.
- **Los ayudantes trabajan en paralelo y a la vista.** Para un viaje, un
  ayudante busca pisos mientras otro revisa los visados y un tercero compara
  vuelos. Navegan por páginas reales que puedes observar, y cualquier fuente se
  abre en un panel junto al lienzo.
- **Los resultados se quedan en el lienzo.** Comparaciones, tablas, gráficos,
  diagramas, planes, código y documentos se presentan donde puedes leerlos. No
  se pierden en el desplazamiento de un chat.
- **Un plan se convierte en tu día.** Un clic transforma cada paso de un plan en
  una tarea vinculada al trabajo de Work. Todo lo que valga la pena conservar
  puede guardarse como nota.
- **Llega más allá del navegador.** Work lee y edita las carpetas a las que le
  das acceso, ejecuta comandos y delega los trabajos de programación más grandes
  en Claude Code o Codex. Se conecta a servidores MCP como Linear, Notion,
  Sentry, Stripe y Figma, y a herramientas de línea de comandos como `gh`.
- **Tú decides.** Todo lo que publique, fusione o modifique algo espera tu
  visto bueno.
- **Usa el modelo que quieras.** Utiliza tus propias claves de Anthropic,
  OpenAI, Google Gemini, DeepSeek u OpenRouter, o ejecuta un modelo en local con
  Ollama, LM Studio o cualquier servidor compatible con OpenAI. Las claves se
  guardan en el llavero de tu sistema.
  O inicia sesión en Zephium AI y olvídate de las claves: elige el modelo
  adecuado para cada paso y empiezas con créditos gratis
  ([precios](https://zephium.app/pricing)).

Work incluye 25 skills, entre ellas planificar un viaje, investigar, comparar y
elegir, planificar mi día, corregir un error y estado semanal. Puedes escribir
las tuyas.

## Ask Zephium

Un asistente en la barra lateral, con `⌘ E` (`Ctrl E` en Windows). Lee la página
en la que estás y responde sobre ella, señala lo que merece tu tiempo y abre en
pestañas nuevas lo que elijas. Pídele que dé unos pasos en la página, o que
cambie el aspecto de un sitio que visitas a diario, y lo hace allí mismo. Cuando
una tarea se queda grande para un chat, **Continue in Work** la lleva al lienzo,
junto con la página.

<p align="center">
  <img src="../../.github/assets/ask.webp" alt="Ask Zephium junto a Hacker News: tres historias elegidas, abiertas en pestañas nuevas, y la portada recompuesta en tarjetas serenas" width="960" />
</p>
<p align="center"><sub>Al preguntarle qué merece la pena leer, elige tres historias, las abre y recompone la portada en tarjetas serenas.</sub></p>

## Browse

### Rápido y ligero

- WebKit nativo en macOS y WebView2 en Windows, de modo que las páginas se
  ejecutan en el motor que tu sistema ya mantiene actualizado.
- Las pestañas inactivas se duermen y se despiertan cuando vuelves a ellas, así
  que el uso de memoria se mantiene bajo aunque tengas muchas pestañas abiertas.
- Un bloqueador nativo de anuncios y rastreadores, activado por defecto,
  construido sobre [adblock-rust](https://github.com/brave/adblock-rust) de
  Brave con EasyList y EasyPrivacy. Las solicitudes se detienen antes de que una
  página pueda hacerlas, y puedes ocultar con un clic cualquier otra cosa de una
  página.

### Pensado para vivir en él

- Una pestaña nueva que se abre al cielo, de día o de noche según tu tema, con
  tu hora, tu concentración y las tareas del día en la parte inferior.
- Pestañas verticales en una barra lateral discreta, con espacios, perfiles,
  pestañas fijadas, carpetas y vista dividida.
- Liquid Glass en macOS 26 y Mica en Windows 11.
- Un lanzador en `⌘ ⇧ Space` (`Ctrl Shift Space` en Windows) llega a pestañas,
  historial, notas y comandos desde cualquier lugar del escritorio. Todo lo que
  escribas ahí puede convertirse en una tarea.
- Todos los atajos de teclado se pueden cambiar.

### Extensiones

Instala extensiones directamente desde la Chrome Web Store. Veinte de las más
populares están verificadas para funcionar bien en Zephium y están a un clic en
el gestor de extensiones, entre ellas 1Password, Bitwarden, Grammarly, DeepL,
Dark Reader, Vimium, SponsorBlock, Raindrop.io, Notion Web Clipper y Refined
GitHub.

### Llévate todo contigo

El asistente de bienvenida importa desde Chrome, Safari, Arc, Zen, Firefox,
Brave y Edge.

## Integrado

<table>
  <tr>
    <td width="33%" align="center"><img src="../../.github/assets/tasks.webp" alt="Tasks: hoy, mañana y próximas, con una tarea vinculada a la página de la que salió" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/time.webp" alt="Time: 3 horas y 24 minutos en la web hoy, una ronda de concentración de 25 minutos y el tiempo por sitio" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/notes.webp" alt="Notes: una lista de comprobación de versión en Markdown con código, junto a la página" /></td>
  </tr>
  <tr>
    <td valign="top"><strong>Tasks</strong><br /><sub>Escríbelas como las dirías, por ejemplo "llamar a Ana mañana a las 3". Listas, prioridades, fechas límite, subtareas, y la página en la que estabas queda vinculada.</sub></td>
    <td valign="top"><strong>Time</strong><br /><sub>Mira en qué se va tu tiempo en la web, contado solo en este dispositivo. Empieza una ronda de Focus y los sitios que elijas permanecen cerrados hasta el descanso.</sub></td>
    <td valign="top"><strong>Notes</strong><br /><sub>Notas junto a la página, cada una un archivo Markdown que es tuyo. Las ediciones hechas desde otras apps aparecen en Zephium.</sub></td>
  </tr>
</table>

## Privado por defecto

- **Sin telemetría.** Zephium no recopila ni envía nada sobre cómo navegas.
- **No necesitas cuenta.** El historial, las tareas, las notas, la memoria y el
  tiempo se quedan en tu dispositivo.
- **Las ventanas privadas no guardan nada** cuando se cierran.

## Descarga

| Plataforma | Arquitectura | Instalador | Requisitos |
| ---------- | ------------ | ---------- | ---------- |
| macOS | Apple Silicon | [`Zephium-macOS-arm64.dmg`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-macOS-arm64.dmg) | macOS Sonoma 14 o posterior |
| Windows | x64 | [`Zephium-Windows-x64-setup.exe`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-Windows-x64-setup.exe) | Windows 10 u 11 |

Las versiones de Windows aún no están firmadas con código, así que SmartScreen
puede mostrar "Windows protegió su PC". Elige **Más información** y después
**Ejecutar de todas formas**. La firma está en camino.

Zephium descarga las actualizaciones en segundo plano y las instala cuando
eliges **Relaunch to update**. Todas las versiones están en la
[página de releases](https://github.com/zephium-browser/Zephium/releases).

## Qué viene

- Linux y Macs con Intel.
- Versiones de Windows firmadas con código.
- Más extensiones verificadas que funcionan y extensiones nativas integradas en Zephium.
- Y mucho más.

## Compilar desde el código fuente

Requisitos previos:

- [Rust](https://rustup.rs) mediante `rustup`. La toolchain fijada en
  [`rust-toolchain.toml`](../../rust-toolchain.toml) se instala sola la primera
  vez que se usa.
- Node.js, en la versión indicada en [`.node-version`](../../.node-version).
- pnpm, mediante Corepack: `corepack enable`.
- Los [requisitos previos de Tauri](https://v2.tauri.app/start/prerequisites/)
  para tu plataforma: Xcode Command Line Tools en macOS, o Microsoft C++ Build
  Tools y WebView2 en Windows.

```sh
git clone https://github.com/zephium-browser/Zephium.git
cd Zephium
pnpm install --frozen-lockfile
pnpm dev
```

Las compilaciones de desarrollo usan su propio perfil `app.zephium.dev`, así que
no tocan los datos de un Zephium instalado. Linux todavía no es compatible.
Consulta [CONTRIBUTING.md](../../CONTRIBUTING.md) antes de abrir un pull request.

## Arquitectura

- Las páginas no confiables se ejecutan en WebViews nativos sin procesar,
  separados de la interfaz privilegiada en Svelte que dibuja el navegador.
- Un núcleo en Rust se encarga de las pestañas, el almacenamiento, el
  bloqueador, las extensiones y los agentes.
- Cada perfil tiene sus propios datos de sitios web aislados, además del
  historial, la sesión y los favicons que gestiona Zephium.
- El bloqueador de red es nativo y está construido sobre
  [adblock-rust](https://github.com/brave/adblock-rust) de Brave.
- Adaptadores internos y acotados para Tauri y Wry gestionan la política de
  almacenamiento, la creación de WebViews, las devoluciones de llamada y el
  cierre.

Más información en [docs/architecture.md](../architecture.md) y
[docs/security-model.md](../security-model.md).

## Contribuir

Zephium lo mantiene una sola persona con una dirección de producto clara, así
que conviene hablar los cambios grandes antes de escribirlos.
[CONTRIBUTING.md](../../CONTRIBUTING.md) explica qué se acepta y cómo. Las
preguntas y las ideas son bienvenidas en [Discord](https://discord.gg/tyveTUyEp7).

## Seguridad

Por favor, informa de las vulnerabilidades de forma privada, no en un issue
público. Consulta [SECURITY.md](../../SECURITY.md).

## Licencia

Zephium se distribuye bajo la [Mozilla Public License 2.0](../../LICENSE).

Las listas de filtros EasyList y EasyPrivacy incluidas se usan bajo
[CC BY-SA 3.0](../../assets/blocker-seed/v1/LICENSE-CC-BY-SA-3.0.txt); consulta
[el aviso](../../assets/blocker-seed/v1/NOTICE) para ver la atribución.

El nombre y el logotipo de Zephium no están licenciados bajo la MPL.

## Agradecimientos

Zephium se apoya en [Tauri](https://tauri.app), [Wry](https://github.com/tauri-apps/wry),
el [adblock-rust](https://github.com/brave/adblock-rust) de Brave y
[EasyList](https://easylist.to).
