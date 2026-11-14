# Refreshing the Chromium schema snapshot

1. Select one reviewed stable tag from the official Chrome Releases announcement.
   Resolve `https://chromium.googlesource.com/chromium/src/+/refs/tags/TAG?format=JSON`
   and record its full commit and tree IDs. Never fetch mutable `HEAD` files or
   mix tags in one snapshot.
2. Start from the paths in `UPSTREAM_FILES.toml`. Review the extension API inventory
   and all cross-namespace `$ref` / `ExternalExtensionType` dependencies. Review
   upstream schema moves and `.json`, `.idl` and `.webidl` dialect changes; retain
   complete API/permission/manifest/behavior metadata and applicable overrides.
   Keep this directory limited to schema/feature data and the upstream `LICENSE`.
3. Fetch each reviewed path at the resolved immutable commit. Gitiles returns
   base64 bytes from `https://chromium.googlesource.com/chromium/src/+/COMMIT/PATH?format=TEXT`.
   Decode those bytes directly into the same relative path. Do not reformat,
   remove comments, alter copyright headers or copy implementation source.
4. Verify each downloaded file's SHA-256 and byte count, then update the exact
   file inventory in `UPSTREAM.md` and `UPSTREAM_FILES.toml`. Record the stable
   release link, tag, commit, tree, retrieval date, license and any local changes.
   Remove obsolete schema files only after dependency review. Review the root
   license for changes and update packaged third-party notice handling if needed.
5. Run `cargo xtask webext-api`, review both generated runtime outputs and regenerate
   the external coverage report with `--coverage PATH --inventory AUDITED_JSON`.
   The inventory maps namespace names to explicitly needed member paths; keep
   guarded, optional, inferred and platform-specific demand distinctions in the
   external evidence rather than treating namespace usage as every member.
   Do not copy description prose into outputs. Run `cargo xtask webext-api --check`
   and the targeted xtask/runtime parser, generator and validator tests, formatting,
   Clippy and dependency/policy checks required by `CONTRIBUTING.md`.
6. Reverify observable errors against implementation source at the same commit;
   keep research implementation files outside the repository. Record newly added
   surface, restrictions, constants, unsupported dialect constructs and untested
   native behavior in review evidence. Native Windows/macOS qualification remains
   separate from parser/generator and source verification.

A refresh is reviewed as one coherent data/provenance/generated-output change.
A failed hash, unknown syntax, missing referenced type or unexplained drift must
be resolved before accepting the new snapshot; preserve the previous pin until
then.
