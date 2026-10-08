# Repository settings

The settings below live in GitHub, not in the code, so they are recorded here.
Everything that can be configured from a file is: CI in
[`workflows/`](workflows/), dependency updates in
[`dependabot.yml`](dependabot.yml), reviews in
[`../.coderabbit.yaml`](../.coderabbit.yaml) and owners in
[`../CODEOWNERS`](../CODEOWNERS).

## Reviews

1. Install the [CodeRabbit GitHub app](https://github.com/apps/coderabbitai)
   for **Only select repositories → Zephium**. It is free for public
   repositories and reads `.coderabbit.yaml`; nothing else needs configuring.
2. Leave CodeRabbit out of the required checks. It skips PRs labelled `slop`
   or `skip-review`, and a skipped required check would block them forever.
   Its pre-merge checks marked `error` still show as failing on the PR.

Commands in a PR comment: `@coderabbitai review` (review now),
`@coderabbitai pause` / `resume`, `@coderabbitai ignore` in the description
(never review this PR), and `@coderabbitai full review` after a large rebase.

## Branch and tag rules

**Settings → Rules → Rulesets → New ruleset → New branch ruleset**

| Field | Value |
| --- | --- |
| Name | `main` |
| Enforcement | Active |
| Bypass list | Repository admin, *For pull requests only* (an emergency merge still goes through a PR) |
| Target branches | Include default branch |
| Restrict deletions | On |
| Block force pushes | On |
| Require a pull request before merging | On, 0 required approvals, *Require conversation resolution* on; allowed merge methods: Merge, Squash |
| Require status checks to pass | On, *Require branches to be up to date* off. Checks: `Changes`, `Workflows`, `Frontend`, `Rust policy`, `Rust (macOS)`, `Rust (Windows)` |

A Rust job that the `Changes` job skips reports *skipped*, which satisfies a
required check, so UI-only PRs are not held up.

**Settings → Rules → Rulesets → New ruleset → New tag ruleset**: name
`releases`, target `v*`, *Restrict creations*, *Restrict updates* and
*Restrict deletions* on, bypass list Repository admin. A `v*` tag starts a
release build, so only a maintainer can push one.

## Pull requests

**Settings → General → Pull Requests**

- Allow merge commits: on, default message *Pull request title*.
- Allow squash merging: on, default message *Pull request title and description*.
- Allow rebase merging: off. Contributor PRs are squashed so the contributor
  stays the author; maintainer PRs with curated commits use a merge commit.
- Always suggest updating pull request branches: on.
- Automatically delete head branches: on.
- Limit open pull requests from users without write access: 3. Trusted
  contributors go on the bypass list.

## Actions

**Settings → Actions → General**

- Fork pull request workflows from outside collaborators: *Require approval
  for all external contributors*. CI only runs on an outside PR after a
  maintainer has looked at it, so spam costs no runner time.
- Workflow permissions: *Read repository contents and packages permissions*,
  and *Allow GitHub Actions to create and approve pull requests* off.

## Security

**Settings → Advanced Security**

- Private vulnerability reporting: on (`SECURITY.md` points to it).
- Dependency graph, Dependabot alerts and Dependabot security updates: on.
- Secret Protection and push protection: on, so a pushed token is rejected
  before it lands.
- Code scanning → CodeQL analysis → *Default setup*: languages Actions,
  JavaScript/TypeScript and Rust; query suite *Default*. It runs on pushes and
  PRs to `main` and weekly.

## Issues and labels

Issue forms are in [`ISSUE_TEMPLATE/`](ISSUE_TEMPLATE/); blank issues are
off and questions go to Discussions. CodeRabbit labels new issues and PRs
from the label set below. `accepted` is applied by hand to a feature issue
once its approach is agreed.

| Group | Labels |
| --- | --- |
| Type | `bug`, `enhancement`, `performance`, `security`, `refactor`, `documentation`, `question` |
| Area | `area: browse`, `area: work`, `area: engine`, `area: blocker`, `area: extensions`, `area: notes & tasks`, `area: build & ci` |
| Platform | `platform: macos`, `platform: windows` |
| Size | `size: S`, `size: M`, `size: L`, `size: XL` |
| Triage | `accepted`, `slop`, `skip-review`, `good first issue`, `help wanted`, `duplicate`, `invalid`, `wontfix` |

## When spam spikes

**Settings → Moderation options → Interaction limits** can restrict
comments, issues and PRs to prior contributors or collaborators for 24 hours
up to six months, and **Moderation options → Blocked users** stops one
account for good.
