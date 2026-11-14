# Chromium extension API schema snapshot

- Repository: <https://chromium.googlesource.com/chromium/src>
- Stable tag: [`157.0.8103.78`](https://chromium.googlesource.com/chromium/src/+/refs/tags/157.0.8103.78)
- Commit: `4b3d0dabb8250b6f152d2d85d79e7706e309d087`
- Tree: `1390557da6bf6cb630528eac294f338b887ad81a`
- Licence: BSD-3-Clause; the unchanged upstream root `LICENSE` is included.
- Upstream files: 47 (678966 bytes), including 33 schemas, 13 feature metadata files and `LICENSE`.
- Local modifications to upstream files: none.

The pin was selected from the [official 11 November 2026 stable release](https://chromereleases.googleblog.com/2026/11/stable-channel-update-for-desktop_01744469280.html).
The tag was resolved through the official Gitiles JSON endpoint; every file was then
downloaded from the immutable commit using HTTPS `?format=TEXT` and base64-decoded
without newline, comment or formatting changes. Retrieval date: 2026-11-14. The topSites schema was added at the same pin;
its Git blob `632076e53a1cc1a5b5e3791e0169a8ace17ab2ac` was checked against
the immutable API-directory tree.

The snapshot covers the API inventory used by the runtime and the transitive
schema references `events`, `extensionTypes` and `types`. Both API roots retain
API, permission and manifest feature files and overrides. The extensions root
also retains behavior feature metadata. Chromium has migrated several required
APIs to `.webidl`; these original definitions are included alongside JSON and
the older extensions `.idl` dialect. Feature files contain entries beyond the
selected surface because they are retained as complete upstream data.

These files are parser input data only. No Chromium implementation source or
schema description prose is copied into generated runtime outputs. This data
snapshot is not a Chromium engine fork or a Cargo dependency, so it adds no
Cargo graph, standalone lockfile or fork patch set. `deny.toml` already permits
BSD-3-Clause; packaged notices include the licence and provenance.

`UPSTREAM_FILES.toml` is the machine-readable equivalent of the following
complete upstream inventory. Paths are relative to this directory. The local
provenance files `UPSTREAM.md`, `UPSTREAM_FILES.toml` and `REBASE.md` are not
upstream files.

| Upstream file | SHA-256 | Bytes |
| --- | --- | ---: |
| `LICENSE` | `368cca1106be99d39ecd32a38d8305585d802a475effb66380b91ffc9bcf709b` | 1536 |
| `chrome/common/extensions/api/_api_features.json` | `f4b7fb88a2f7fd91d9e8c453cf362aeef2af35d9d53027920ca816440cb3132b` | 31365 |
| `chrome/common/extensions/api/_api_features.override.json` | `d9b6ca011bffb30b3e6e22c9e0cbc2a7da55907f6102a0e4bb7bc3780830a214` | 1512 |
| `chrome/common/extensions/api/_manifest_features.json` | `bb7ac1a56f963a2c09f7f531a9f0751b5e47d24918e9a985bc4160f1515b9722` | 1719 |
| `chrome/common/extensions/api/_manifest_features.override.json` | `6daa867304d55e98b0224b3ae7837ecbe058c6bbc32a1f4839646ebe9aa80293` | 576 |
| `chrome/common/extensions/api/_permission_features.json` | `c14bd0e35e89096f68358f95a9118b9d7dd60d026a6cc2fea4c6c1c3ef5fd8f9` | 43437 |
| `chrome/common/extensions/api/_permission_features.override.json` | `09c45add6f653cfd086d9cfa114ba196b8f0444aec3dc411284f41c6563eb39d` | 578 |
| `chrome/common/extensions/api/action.json` | `b7ae92c0449c015f3990939b2aff7be545523a8bd9ccc40ede378e8e5cca90b7` | 15102 |
| `chrome/common/extensions/api/bookmarks.json` | `a7c3baa648467c8b83d0fd57ed785f31f9c31769f6ff6bf12790c20a51bcc98b` | 18322 |
| `chrome/common/extensions/api/commands.json` | `8c1cc11b75b8c4ee922a3a6d71ebabeeb17b7ec24cbe5b822ce68fd3521d907b` | 2249 |
| `chrome/common/extensions/api/context_menus.json` | `8ce169f56f255ed41cb51ae821e113434183da3b866b46ef6c33aeb3e2f0ae48` | 14622 |
| `chrome/common/extensions/api/downloads.webidl` | `d62d30d1ab4407bdea1d1c600d076313183fe3aa82bf3641d4e5a426018cbb13` | 25750 |
| `chrome/common/extensions/api/extension.json` | `89de731501c370420c3d9964dd5d0876e0ecb09383ffa4ed5dc1cb19ce987294` | 9448 |
| `chrome/common/extensions/api/font_settings.json` | `d38b8aa2debe69c7b528a0f55d11e36c7c1027f14645d2fc04da7590437a1781` | 16177 |
| `chrome/common/extensions/api/history.json` | `292fccf51cbf57e57274db659b5a82df55785572f3b82c863c53c114f39fddc0` | 9893 |
| `chrome/common/extensions/api/notifications.webidl` | `fd6665813b27959ada75e68c6691fd1ca8c47b025d574f48a1f09eb8608aeced` | 9210 |
| `chrome/common/extensions/api/privacy.json` | `540ed37792ef41d4440bc19704e23eba7ded3363f651bcb952717b066e3cef3a` | 12159 |
| `chrome/common/extensions/api/search.idl` | `e0643988ce8dce3f968f03bdba046abbd6ff1a686031d60281b2fb55af5ba0b4` | 1232 |
| `chrome/common/extensions/api/sessions.json` | `a81c446b0d4768e572c39eb5ea2834ef3dc2bb6f762bde85c273e82f52cca52a` | 4813 |
| `chrome/common/extensions/api/side_panel.idl` | `92e7591b8f4560567978002066b24ae36293890947c4a2207eb2e9e83ad5bfa5` | 7716 |
| `chrome/common/extensions/api/tabs.json` | `5380b79db1c0078e99ca5f97f55bde46d357e07d76d3afcf886020c69a8c4b23` | 61413 |
| `chrome/common/extensions/api/top_sites.json` | `163c6ed6731e9fbb8f671cacbe8e4a5517abdd47a57581db8b9f257a887bc5a0` | 1213 |
| `chrome/common/extensions/api/web_navigation.json` | `fc55f4b97176d7c45e0dfcd59bb67753eecc37d664eb0d1aefe3045d31c1727b` | 24234 |
| `chrome/common/extensions/api/windows.json` | `8825ea2a367f892ce76c58931b1d2da3c2408bbbed751ec3ce727d1f9597e8de` | 18268 |
| `extensions/common/api/_api_features.json` | `6157faf0bdd37519175567875a004f4a848d54d6a05d4c55a2b19490ba7f3324` | 34390 |
| `extensions/common/api/_api_features.override.json` | `d16cb4b617a23a186216cb24dd8ad0aabc6fd62c67e7fb169d5aefd0cd07a427` | 571 |
| `extensions/common/api/_behavior_features.json` | `4b2e8c3a5eaa954f084ca153f9c830c33a6594982deb772117e01260cad6d081` | 12254 |
| `extensions/common/api/_manifest_features.json` | `9f0c66dd258c5a5ff01956c11dc240065f1878a591ffa64ebbc6fbbe73ecc123` | 20359 |
| `extensions/common/api/_manifest_features.override.json` | `6daa867304d55e98b0224b3ae7837ecbe058c6bbc32a1f4839646ebe9aa80293` | 576 |
| `extensions/common/api/_permission_features.json` | `cd185b5ea3321ed87e0ec8774b2379e402dbb829adb31982f4b2c99cc4904ae2` | 36287 |
| `extensions/common/api/_permission_features.override.json` | `09c45add6f653cfd086d9cfa114ba196b8f0444aec3dc411284f41c6563eb39d` | 578 |
| `extensions/common/api/alarms.webidl` | `d3ceb0f00b78663ed503da2c25774b3ac2a82c7857190b60969def5d96a79d9d` | 4508 |
| `extensions/common/api/declarative_net_request.webidl` | `7f91a8526b3aa87c6900b4e70d76ba1127a2d9a600214e0e0df15214a2f0c53f` | 36791 |
| `extensions/common/api/dom.json` | `83b6fcf8f0298a69ff2d4ad0b2e07ab3bb97f2547e9757a98a7f91fba69308d1` | 1096 |
| `extensions/common/api/events.json` | `ed719a83acd3042bdb62190fe6a9ea02364f6b848e747231e9b7eaa75b44f9f6` | 14353 |
| `extensions/common/api/extension_types.json` | `ab82c757dde2e8539141f117edce8fe8d2141a1b1b4ffdbcb55c7e80cddb9671` | 9719 |
| `extensions/common/api/i18n.json` | `eb56cdf3ee3d264bbb00feae4b78f1b3e632cfaee80ca005acc8012a2049379e` | 5307 |
| `extensions/common/api/idle.json` | `bfdac0618ed2aaa6a01c775936153c5093ecbc6ec0861410cd31dd33a6aafa66` | 3038 |
| `extensions/common/api/management.json` | `fd25144ed2050443643497099e65b3b978f415dd09d89c4e78119e59801b7c2d` | 16639 |
| `extensions/common/api/offscreen.webidl` | `a3c64f457d0d6deb2f754beffba141c239afd0164f558d350aa59e179045c25a` | 3805 |
| `extensions/common/api/permissions.json` | `56266a7c00ea2d65264971f71188bb105744b8182ba31883572bb20cc1acd697` | 8390 |
| `extensions/common/api/runtime.json` | `3ff17ed602ccf647d2200a2f7cd1cbcbb5bfcc35f41cd58ce68de484c1fb2f81` | 46933 |
| `extensions/common/api/scripting.webidl` | `066b2f352aa71d1816dee6c0ab98a12a7013e6bd117c01b9f8373bc6b8f15b76` | 10478 |
| `extensions/common/api/storage.json` | `54a24ce3f49d14121b8df9aaac18ad72ce3d88dd5ff8b748c483577a706c3a9e` | 13760 |
| `extensions/common/api/types.json` | `e0eed741e4b912d1ef1e8c38781b5bbe3f9c2aef96293ce609d416e11244660f` | 6984 |
| `extensions/common/api/user_scripts.webidl` | `44e4717d50ae5a9bbf88903d89aa087928cfc10812ce9009237df4f4f5d1a6bc` | 9524 |
| `extensions/common/api/web_request.json` | `329b698fbfb894853f1ecd7120ffd3b845d9f1d0fdb82aeef05affcf78d6c12d` | 50082 |
