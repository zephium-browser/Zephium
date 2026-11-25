# Feat: The CLI can't select a profile at launch

[Orion gives each profile its own browser and Dock icon](https://orionbrowser.com/platforms/macos). Zephium could use `--profile <name>` to open a profile at launch.

`zephium --profile work` should open the existing profile or create it on first use. The selection applies to that launch only, leaving the saved default unchanged. Zephium should report an error and exit if the flag has no value or appears more than once, the name is invalid, or Zephium can't create or open the profile.
