<div align="center">
  <img src="../../.github/assets/logo.png" width="112" height="112" alt="Zephium" />
  <h1>Zephium</h1>

  <p><strong>Um navegador e um ambiente de trabalho rápidos e completos,<br />reimaginados para você e seus agentes.</strong></p>

  <p>
    <a href="https://github.com/zephium-browser/Zephium/releases"><img src="https://img.shields.io/github/v/release/zephium-browser/Zephium?include_prereleases&label=release&color=blue" alt="Última versão" /></a>
    <a href="../../LICENSE"><img src="https://img.shields.io/badge/license-MPL--2.0-blue" alt="Licença: MPL-2.0" /></a>
    <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey" alt="Plataformas: macOS e Windows" />
    <a href="https://discord.gg/tyveTUyEp7"><img src="https://img.shields.io/badge/Discord-5865F2?logo=discord&logoColor=white" alt="Discord" /></a>
    <a href="https://www.youtube.com/@crynta"><img src="https://img.shields.io/badge/YouTube-FF0000?logo=youtube&logoColor=white" alt="YouTube" /></a>
    <a href="https://x.com/CryntaDev"><img src="https://img.shields.io/badge/x.com-000000?logo=x&logoColor=white" alt="X: @CryntaDev" /></a>
  </p>

  <p>
    <a href="https://zephium.app">Site</a> ·
    <a href="https://zephium.app/docs">Documentação</a> ·
    <a href="https://youtu.be/QbSarcWspvw">Filme de lançamento</a> ·
    <a href="https://github.com/zephium-browser/Zephium/releases/latest">Baixar</a>
  </p>
</div>

<details align="center">
  <summary><sub>Ler em outro idioma</sub></summary>
  <sub>
    <a href="../../README.md">English</a> ·
    <a href="README.zh-CN.md">简体中文</a> ·
    <a href="README.ja.md">日本語</a> ·
    <a href="README.ko.md">한국어</a> ·
    <a href="README.hi.md">हिन्दी</a> ·
    <a href="README.es.md">Español</a> ·
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
    <img src="../../.github/assets/browse-light.webp" alt="A nova aba do Zephium: a janela se abre para o céu, com o nome, a hora e a data, e os números do dia na parte de baixo" width="960" />
  </picture>
</p>

<p align="center">
  <strong>A eficiência do Safari. A proteção do Brave. O design do Arc.</strong><br />
  E o <strong>Work</strong>, uma tela onde seus agentes fazem trabalho de verdade, à vista de todos.
</p>

<p align="center"><a href="https://youtu.be/QbSarcWspvw">▶&nbsp; Assistir ao filme de lançamento</a></p>

---

O Zephium é um navegador de código aberto, feito em Rust sobre o mecanismo web
do próprio sistema operacional: WebKit no macOS, o mesmo mecanismo do Safari, e
WebView2 no Windows. Não é um fork do Chromium nem do Firefox. Ele bloqueia
anúncios e rastreadores de forma nativa, executa extensões do Chrome e deixa a
um clique as tarefas, as notas e o seu tempo na web. O download tem cerca de
30 MB.

A um só interruptor de distância está o **Work**. Seu trabalho já acontece no
navegador, onde estão suas abas, seus logins e seu histórico. O Work leva os
agentes até lá, em vez de pedir que você mude para outro lugar, e deixa você
acompanhar tudo o que eles fazem.

## Work

Descreva um resultado com suas próprias palavras: planejar uma viagem, comparar
fornecedores, desenhar um sistema, corrigir um bug. O Work assume a partir daí,
em uma tela, e você vê cada passo.

<p align="center">
  <img src="../../.github/assets/work.webp" alt="Uma execução completa do Work no Zephium: ajudantes lendo Uptime Kuma, Gatus e OpenStatus nos próprios sites, um ajudante local verificando os endpoints e, depois, a resposta, uma tabela comparativa com gráficos, um diagrama, a configuração e quatro tarefas" width="960" />
</p>
<p align="center"><sub>Ao receber o pedido de uma página de status, o Work lê três monitores nos próprios sites, verifica seus endpoints a partir da sua máquina, monta a escolha e deixa na tela a comparação, a configuração e as tarefas.</sub></p>

- **Ele parte do que você já tem.** O Work lembra o que sabe sobre você, busca
  no seu histórico quando isso ajuda e carrega a skill certa para cada tarefa.
  Cada passo aparece na tela conforme acontece.
- **Os ajudantes trabalham em paralelo, às claras.** Numa viagem, um ajudante
  procura apartamentos enquanto outro confere vistos e um terceiro compara
  voos. Eles navegam por páginas reais que você pode acompanhar, e qualquer
  fonte abre em um painel ao lado da tela.
- **Os resultados ficam na tela.** Comparações, tabelas, gráficos, diagramas,
  planos, código e documentos são organizados onde você consegue ler. Não se
  perdem na rolagem de um chat.
- **Um plano vira o seu dia.** Um clique transforma cada passo de um plano em
  uma tarefa ligada ao Work de origem. Tudo o que vale guardar pode ser salvo
  como nota.
- **Vai além do navegador.** O Work lê e edita as pastas que você liberar,
  executa comandos e entrega tarefas maiores de programação ao Claude Code ou ao
  Codex. Ele se conecta a servidores MCP como Linear, Notion, Sentry, Stripe e
  Figma, e a ferramentas de linha de comando como `gh`.
- **Quem decide é você.** Tudo o que publica, faz merge ou altera algo espera o
  seu OK.
- **Use o modelo que quiser.** Use suas próprias chaves da Anthropic, OpenAI,
  Google Gemini, DeepSeek ou OpenRouter, ou rode um modelo localmente com
  Ollama, LM Studio ou qualquer servidor compatível com a OpenAI. As chaves
  ficam no chaveiro do seu sistema.
  Ou entre no Zephium AI e dispense as chaves: ele escolhe o modelo certo para
  cada etapa, e você começa com créditos grátis
  ([preços](https://zephium.app/pricing)).

O Work vem com 25 skills, incluindo planejar viagem, pesquisar, comparar e
escolher, planejar meu dia, corrigir um bug e status semanal. Você pode criar as
suas.

## Ask Zephium

Um assistente na barra lateral, em `⌘ E` (`Ctrl E` no Windows). Ele lê a página
em que você está e responde sobre ela, aponta o que vale o seu tempo e abre em
novas abas o que você escolher. Peça que ele faça alguns passos na página, ou que
mude o visual de um site que você visita todo dia, e ele faz ali mesmo. Quando
uma tarefa fica grande demais para um chat, **Continue in Work** a leva para a
tela, junto com a página.

<p align="center">
  <img src="../../.github/assets/ask.webp" alt="Ask Zephium ao lado do Hacker News: três matérias escolhidas, abertas em novas abas, e a página inicial reorganizada em cartões tranquilos" width="960" />
</p>
<p align="center"><sub>Ao ser perguntado o que vale a pena ler, ele escolhe três matérias, abre todas e reorganiza a página inicial em cartões tranquilos.</sub></p>

## Browse

### Rápido e leve

- WebKit nativo no macOS e WebView2 no Windows, para que as páginas rodem no
  mecanismo que seu sistema já mantém atualizado.
- As abas inativas entram em repouso e despertam quando você volta a elas, então
  o uso de memória continua baixo mesmo com muitas abas abertas.
- Um bloqueador nativo de anúncios e rastreadores, ativado por padrão, baseado
  no [adblock-rust](https://github.com/brave/adblock-rust) do Brave, com
  EasyList e EasyPrivacy. As requisições são barradas antes que uma página possa
  fazê-las, e você pode ocultar com um clique qualquer outra coisa em uma
  página.

### Feito para morar nele

- Uma nova aba que se abre para o céu, de dia ou de noite conforme o seu tema,
  com a hora, o seu foco e as tarefas do dia na parte de baixo.
- Abas verticais em uma barra lateral discreta, com espaços, perfis, abas
  fixadas, pastas e divisão de tela.
- Liquid Glass no macOS 26 e Mica no Windows 11.
- Um lançador em `⌘ ⇧ Space` (`Ctrl Shift Space` no Windows) alcança abas,
  histórico, notas e comandos de qualquer lugar da sua área de trabalho. Tudo o
  que você digitar ali pode virar uma tarefa.
- Todo atalho de teclado pode ser alterado.

### Extensões

Instale extensões direto da Chrome Web Store. Vinte das mais populares foram
verificadas e funcionam bem no Zephium, e estão a um clique no gerenciador de
extensões, incluindo 1Password, Bitwarden, Grammarly, DeepL, Dark Reader,
Vimium, SponsorBlock, Raindrop.io, Notion Web Clipper e Refined GitHub.

### Leve tudo com você

O fluxo de boas-vindas importa dados do Chrome, Safari, Arc, Zen, Firefox, Brave
e Edge.

## Já incluído

<table>
  <tr>
    <td width="33%" align="center"><img src="../../.github/assets/tasks.webp" alt="Tasks: hoje, amanhã e próximas, com uma tarefa vinculada à página de onde veio" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/time.webp" alt="Time: 3 horas e 24 minutos na web hoje, uma rodada de foco de 25 minutos e o tempo por site" /></td>
    <td width="33%" align="center"><img src="../../.github/assets/notes.webp" alt="Notes: um checklist de release em Markdown com código, ao lado da página" /></td>
  </tr>
  <tr>
    <td valign="top"><strong>Tasks</strong><br /><sub>Escreva do jeito que você falaria, como "ligar para a Ana amanhã às 15h". Listas, prioridades, prazos, subtarefas, e a página em que você estava fica vinculada.</sub></td>
    <td valign="top"><strong>Time</strong><br /><sub>Veja para onde vai o seu tempo na web, contado somente neste dispositivo. Inicie uma rodada de Focus e os sites que você escolher ficam fechados até o intervalo.</sub></td>
    <td valign="top"><strong>Notes</strong><br /><sub>Notas ao lado da página, cada uma um arquivo Markdown que é seu. As edições feitas em outros apps aparecem no Zephium.</sub></td>
  </tr>
</table>

## Privado por padrão

- **Sem telemetria.** O Zephium não coleta nem envia nada sobre como você navega.
- **Sem necessidade de conta.** Histórico, tarefas, notas, memória e tempo ficam
  no seu dispositivo.
- **Janelas privadas não guardam nada** depois de fechadas.

## Download

| Plataforma | Arquitetura | Instalador | Requisitos |
| ---------- | ----------- | ---------- | ---------- |
| macOS | Apple Silicon | [`Zephium-macOS-arm64.dmg`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-macOS-arm64.dmg) | macOS Sonoma 14 ou posterior |
| Windows | x64 | [`Zephium-Windows-x64-setup.exe`](https://github.com/zephium-browser/Zephium/releases/latest/download/Zephium-Windows-x64-setup.exe) | Windows 10 ou 11 |

As versões para Windows ainda não têm assinatura de código, então o SmartScreen
pode exibir "O Windows protegeu o seu computador". Escolha **Mais informações** e
depois **Executar assim mesmo**. A assinatura está a caminho.

O Zephium baixa as atualizações em segundo plano e as instala quando você
escolhe **Relaunch to update**. Todas as versões estão na
[página de releases](https://github.com/zephium-browser/Zephium/releases).

## O que vem por aí

- Linux e Macs com Intel.
- Versões para Windows com assinatura de código.
- Mais extensões verificadas e funcionando, e extensões nativas integradas ao Zephium.
- E muito mais.

## Compilar a partir do código-fonte

Pré-requisitos:

- [Rust](https://rustup.rs) por meio do `rustup`. A toolchain fixada em
  [`rust-toolchain.toml`](../../rust-toolchain.toml) se instala sozinha no
  primeiro uso.
- Node.js, na versão indicada em [`.node-version`](../../.node-version).
- pnpm, por meio do Corepack: `corepack enable`.
- Os [pré-requisitos do Tauri](https://v2.tauri.app/start/prerequisites/) para a
  sua plataforma: Xcode Command Line Tools no macOS, ou Microsoft C++ Build
  Tools e WebView2 no Windows.

```sh
git clone https://github.com/zephium-browser/Zephium.git
cd Zephium
pnpm install --frozen-lockfile
pnpm dev
```

As compilações de desenvolvimento usam o próprio perfil `app.zephium.dev`, então
não mexem nos dados de um Zephium instalado. O Linux ainda não é compatível.
Leia o [CONTRIBUTING.md](../../CONTRIBUTING.md) antes de abrir um pull request.

## Arquitetura

- Páginas não confiáveis rodam em WebViews nativas puras, separadas da interface
  privilegiada em Svelte que desenha o navegador.
- Um núcleo em Rust cuida das abas, do armazenamento, do bloqueador, das
  extensões e dos agentes.
- Cada perfil tem seus próprios dados de sites isolados, além de histórico,
  sessão e favicons gerenciados pelo Zephium.
- O bloqueador de rede é nativo e baseado no
  [adblock-rust](https://github.com/brave/adblock-rust) do Brave.
- Adaptadores internos e enxutos para Tauri e Wry cuidam da política de
  armazenamento, da criação das WebViews, dos callbacks e do encerramento.

Saiba mais em [docs/architecture.md](../architecture.md) e
[docs/security-model.md](../security-model.md).

## Contribuindo

O Zephium é mantido por uma pessoa, com uma direção de produto clara, então
converse sobre mudanças maiores antes de escrevê-las.
O [CONTRIBUTING.md](../../CONTRIBUTING.md) explica o que é aceito e como.
Perguntas e ideias são bem-vindas no [Discord](https://discord.gg/tyveTUyEp7).

## Segurança

Por favor, reporte vulnerabilidades de forma privada, não em uma issue pública.
Veja o [SECURITY.md](../../SECURITY.md).

## Licença

O Zephium é licenciado sob a [Mozilla Public License 2.0](../../LICENSE).

As listas de filtros EasyList e EasyPrivacy incluídas são usadas sob a
[CC BY-SA 3.0](../../assets/blocker-seed/v1/LICENSE-CC-BY-SA-3.0.txt); veja
[o aviso](../../assets/blocker-seed/v1/NOTICE) para a atribuição.

O nome e o logotipo do Zephium não são licenciados sob a MPL.

## Agradecimentos

O Zephium se apoia em [Tauri](https://tauri.app), [Wry](https://github.com/tauri-apps/wry),
no [adblock-rust](https://github.com/brave/adblock-rust) do Brave e na
[EasyList](https://easylist.to).
