# WebView2 security review: October 6, 2026

The reviewed floor and latest recommendation are now **154.0.4258.62**,
published October 5. Review again by October 13 inclusive; CI expires at
2026-10-14T00:00:00Z. **Production releases are blocked** on a pending
Microsoft security fix announced October 6 (below); runtime admission is
unaffected.

## Vendor evidence

[Microsoft's security release notes](https://learn.microsoft.com/en-us/deployedge/microsoft-edge-relnotes-security)
list Stable 154.0.4258.62 on October 5 as incorporating the latest Chromium
security updates, with CVE details to follow. An October 6 entry then states
that Microsoft is aware of recent Chromium security fixes and is working on a
security fix. Under Zephium's policy that notice sets
`PRODUCTION_RELEASE_BLOCKED_ON_OUTSTANDING_VENDOR_FIX`: publication waits
until Microsoft ships a Stable release after October 6 and the floor moves to
it.

[Stable release notes](https://learn.microsoft.com/en-us/deployedge/microsoft-edge-relnote-stable-channel)
confirm 154.0.4258.62 as the October 5 Stable update. Extended Stable
152.0.4191.119 is a different servicing line, not an exception to the
Evergreen floor.

The [Microsoft Update Catalog query](https://www.catalog.update.microsoft.com/Search.aspx?q=Microsoft+WebView2+Runtime+154.0.4258.62)
returns WebView2 Runtime 154.0.4258.62 packages for ARM64, x86 and x64, each
last updated October 5.

Edge now ships a new Stable major about every two weeks; a 155 runtime keeps
the unreviewed-runtime advisory until it is reviewed. Beta, Dev and Canary
remain denied.

## Boundary

This was a vendor-source review only. It did not reinstall or inventory a
Windows machine; 154.0.4258.53 and older runtimes now start with an
update-recommended advisory. Packaged native qualification and a second
maintainer's review remain required before signing a release.
