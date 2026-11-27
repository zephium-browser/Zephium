# Launch with a profile

Append `--profile <name>` to the Zephium launch command to open a specific
profile without changing the saved default profile:

```sh
<Zephium executable> --profile Work
```

The `--profile=<name>` form is also accepted. If the named profile exists,
Zephium opens it. If it does not exist, Zephium creates it on first use. Without
`--profile`, startup uses the active profile or falls back to the saved default.

The option may be supplied once and requires a valid, non-empty profile name.
An invalid or unavailable profile stops startup with an error; Zephium does not
silently continue in another profile.
