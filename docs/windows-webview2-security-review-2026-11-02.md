# WebView2 security review: November 2, 2026

The reviewed floor is now **156.0.4314.56** (published October 29) and the
latest recommendation **156.0.4314.58** (October 30). Review again by
November 9 inclusive; CI expires at 2026-11-10T00:00:00Z. Runtime admission is
unaffected.

## Vendor evidence

[Microsoft's security release notes](https://learn.microsoft.com/en-us/deployedge/microsoft-edge-relnotes-security)
list two Stable security releases after the October 24 review:
156.0.4314.56 (October 29) and 156.0.4314.58 (October 30), each stating that
it incorporates the latest Chromium security updates, with CVE details to
follow. The page carries no exploited-in-the-wild entry and no notice of a
pending fix in that range. The November 2 entry is Edge for Android and iOS
only.

[Stable release notes](https://learn.microsoft.com/en-us/deployedge/microsoft-edge-relnote-stable-channel)
list 156.0.4314.50 (October 26, Update 1), 156.0.4314.56 (October 29,
Update 2) and 156.0.4314.58 (October 30, Update 3).

The [Microsoft Update Catalog query](https://www.catalog.update.microsoft.com/Search.aspx?q=Microsoft+WebView2+Runtime+156.0.4314.56)
returns WebView2 Runtime 156.0.4314.56 packages for x86, x64 and ARM64, each
last updated 10/29/2026. The same query for 156.0.4314.58 returns nothing
yet, so the floor stays at the newest release with catalog packages and
156.0.4314.58 is the recommendation: Evergreen runtimes already receive it
(a Windows 11 test machine runs 156.0.4314.58).

## Boundary

This was a vendor-source review only. 156.0.4314.42 and older runtimes now
start with an update-recommended advisory. Packaged native qualification and a
second maintainer's review remain required before signing a release.
